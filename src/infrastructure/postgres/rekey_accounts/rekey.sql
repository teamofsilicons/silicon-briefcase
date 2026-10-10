-- Reuse link-identities' transaction-scoped maintenance helpers: exclusive
-- table locks, deferred FKs, exact restoration of triggers and RLS policies.
SELECT pg_advisory_xact_lock(hashtextextended('briefcase-account-uuid-backfill', 0));
SELECT pg_temp.link_maintenance_on();
SELECT pg_temp.link_check_actor_columns();

CREATE TEMP TABLE uuid_refs (
    table_name text, column_name text, kind_column text, expected_kind text,
    PRIMARY KEY (table_name, column_name)
) ON COMMIT DROP;
INSERT INTO uuid_refs SELECT table_name,id_column,kind_column,NULL FROM link_actor_columns;
INSERT INTO uuid_refs VALUES
    ('accounts','account_uuid','kind',NULL),
    ('accounts','custodian_uuid',NULL,'carbon'),
    ('account_id_history','account_uuid',NULL,NULL),
    ('accounts_webhook_receipts','account_uuid',NULL,NULL),
    ('organizations','owner_account_uuid',NULL,NULL),
    ('silicon_share_allowances','silicon_uuid',NULL,'silicon'),
    ('silicon_share_allowances','allowed_uuid',NULL,NULL),
    ('silicon_share_allowances','allowed_by_uuid',NULL,NULL),
    ('identity_links','accounts_uuid','identity_kind',NULL),
    ('legacy_path_aliases','drive',NULL,NULL),
    ('testing_environment_idempotency','authority_id','authority_type',NULL);
INSERT INTO uuid_refs
SELECT column_row.table_name,column_row.column_name,NULL,NULL
FROM information_schema.columns column_row
JOIN information_schema.tables table_row USING(table_catalog,table_schema,table_name)
WHERE column_row.table_schema='briefcase' AND table_row.table_type='BASE TABLE'
  AND column_row.column_name='org_id' AND column_row.table_name <> 'identity_link_ledger';

CREATE TEMP TABLE uuid_used (old_uuid text PRIMARY KEY) ON COMMIT DROP;
CREATE TEMP TABLE uuid_counts (reference text PRIMARY KEY, changed bigint NOT NULL) ON COMMIT DROP;
CREATE TEMP TABLE uuid_fresh ON COMMIT DROP AS
    SELECT input.* FROM uuid_input input
    WHERE NOT EXISTS (SELECT 1 FROM briefcase.account_uuid_mappings prior WHERE prior.old_uuid=input.old_uuid);

DO $$
DECLARE item record; invalid boolean; predicate text; unknown text;
BEGIN
    IF EXISTS (SELECT 1 FROM briefcase.accounts account
                WHERE account.account_uuid !~ '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
                  AND NOT EXISTS (SELECT 1 FROM uuid_input input WHERE input.old_uuid=account.account_uuid)) THEN
        RAISE EXCEPTION 'mapping is incomplete: a legacy cached Accounts UUID is missing';
    END IF;
    IF EXISTS (SELECT 1 FROM uuid_input input JOIN briefcase.account_uuid_mappings prior
                   ON input.old_uuid=prior.old_uuid OR input.new_uuid=prior.new_uuid
               WHERE (input.old_uuid,input.new_uuid,input.kind) IS DISTINCT FROM (prior.old_uuid,prior.new_uuid,prior.kind)) THEN
        RAISE EXCEPTION 'mapping conflicts with an immutable, previously applied account mapping';
    END IF;
    IF EXISTS (SELECT 1 FROM uuid_input input JOIN briefcase.account_uuid_mappings prior
                   ON input.old_uuid=prior.new_uuid OR input.new_uuid=prior.old_uuid) THEN
        RAISE EXCEPTION 'mapping would form a chain or swap with a previously applied mapping';
    END IF;
    IF EXISTS (SELECT 1 FROM briefcase.organizations drive JOIN uuid_input input ON drive.org_id=input.old_uuid
               WHERE drive.owner_account_uuid IS DISTINCT FROM drive.org_id) THEN
        RAISE EXCEPTION 'an old account UUID collides with a legacy organization; resolve its identity links first';
    END IF;
    SELECT string_agg(column_row.table_name || '.' || column_row.column_name, ', ') INTO unknown
      FROM information_schema.columns column_row JOIN information_schema.tables table_row USING(table_catalog,table_schema,table_name)
     WHERE column_row.table_schema='briefcase' AND table_row.table_type='BASE TABLE'
       AND column_row.column_name IN ('account_uuid','accounts_uuid','custodian_uuid','owner_account_uuid','silicon_uuid','allowed_uuid','allowed_by_uuid')
       AND NOT EXISTS (SELECT 1 FROM uuid_refs ref WHERE ref.table_name=column_row.table_name AND ref.column_name=column_row.column_name);
    IF unknown IS NOT NULL THEN RAISE EXCEPTION 'unreviewed identity columns: %', unknown; END IF;

    FOR item IN SELECT * FROM uuid_refs ORDER BY table_name,column_name LOOP
        predicate := CASE WHEN item.kind_column IS NULL THEN 'true'
            ELSE format('row.%I IN (''carbon'',''silicon'')',item.kind_column) END;
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM briefcase.%I row JOIN uuid_fresh input ON row.%I=input.new_uuid WHERE %s)',
                       item.table_name,item.column_name,predicate) INTO invalid;
        IF invalid THEN RAISE EXCEPTION 'new UUID already owns data in %.%; refusing identity merge',item.table_name,item.column_name; END IF;
        IF item.kind_column IS NOT NULL THEN
            EXECUTE format('SELECT EXISTS (SELECT 1 FROM briefcase.%I row JOIN uuid_input input ON row.%I=input.old_uuid WHERE %s AND row.%I <> input.kind)',
                           item.table_name,item.column_name,predicate,item.kind_column) INTO invalid;
        ELSIF item.expected_kind IS NOT NULL THEN
            EXECUTE format('SELECT EXISTS (SELECT 1 FROM briefcase.%I row JOIN uuid_input input ON row.%I=input.old_uuid WHERE input.kind <> %L)',
                           item.table_name,item.column_name,item.expected_kind) INTO invalid;
        ELSE invalid := false;
        END IF;
        IF invalid THEN RAISE EXCEPTION 'account kind disagrees with %.%',item.table_name,item.column_name; END IF;
        EXECUTE format('INSERT INTO uuid_used SELECT DISTINCT input.old_uuid FROM briefcase.%I row JOIN uuid_input input ON row.%I=input.old_uuid WHERE %s ON CONFLICT DO NOTHING',
                       item.table_name,item.column_name,predicate);
    END LOOP;
END $$;

-- Only structurally identified identity values are rewritten. Free-form
-- content, URLs, public IDs, private resource IDs and storage paths survive.
CREATE FUNCTION pg_temp.rekey_identity_json(value jsonb) RETURNS jsonb LANGUAGE plpgsql STABLE AS $$
DECLARE pair record; result jsonb; replacement text; typed boolean;
BEGIN
    IF jsonb_typeof(value)='array' THEN
        SELECT COALESCE(jsonb_agg(pg_temp.rekey_identity_json(element)), '[]'::jsonb)
          INTO result FROM jsonb_array_elements(value) element;
        RETURN result;
    END IF;
    IF jsonb_typeof(value)<>'object' OR value IS NULL THEN RETURN value; END IF;
    typed := COALESCE(value->>'kind', value->>'type') IN ('carbon','silicon');
    result := '{}'::jsonb;
    FOR pair IN SELECT * FROM jsonb_each(value) LOOP
        replacement := NULL;
        IF jsonb_typeof(pair.value)='string' AND (
            pair.key IN ('account_uuid','accounts_uuid','custodian_uuid','owner_account_uuid','silicon_uuid','allowed_uuid','allowed_by_uuid',
                         'org_id','drive','owner_id','actor_id','created_by_id','updated_by_id','principal_id','granted_by_id','revoked_by_id',
                         'requested_by_id','decided_by_id','recipient_id','authority_id')
            OR (pair.key IN ('id','uuid') AND typed)) THEN
            SELECT new_uuid INTO replacement FROM uuid_input WHERE old_uuid=pair.value #>> '{}';
        END IF;
        result := result || jsonb_build_object(pair.key, COALESCE(to_jsonb(replacement),pg_temp.rekey_identity_json(pair.value)));
    END LOOP;
    RETURN result;
END $$;

DO $$
DECLARE item record; changed bigint; predicate text;
BEGIN
    -- The personal-drive consistency CHECK is immediate, so change both keys
    -- in the same statement. All referencing FKs remain deferred.
    UPDATE briefcase.organizations drive SET org_id=input.new_uuid,owner_account_uuid=input.new_uuid
      FROM uuid_input input WHERE drive.org_id=input.old_uuid AND drive.owner_account_uuid=input.old_uuid;
    GET DIAGNOSTICS changed=ROW_COUNT;
    INSERT INTO uuid_counts VALUES ('organizations.org_id+owner_account_uuid',changed);
    FOR item IN SELECT * FROM uuid_refs WHERE table_name<>'organizations' ORDER BY table_name,column_name LOOP
        predicate := CASE WHEN item.kind_column IS NULL THEN 'true'
            ELSE format('row.%I=input.kind',item.kind_column) END;
        EXECUTE format('UPDATE briefcase.%I row SET %I=input.new_uuid FROM uuid_input input WHERE row.%I=input.old_uuid AND %s',
                       item.table_name,item.column_name,item.column_name,predicate);
        GET DIAGNOSTICS changed=ROW_COUNT;
        INSERT INTO uuid_counts VALUES (item.table_name || '.' || item.column_name,changed);
    END LOOP;
    FOR item IN SELECT * FROM (VALUES
        ('audit_events','metadata'),('notifications','details'),
        ('idempotency_records','response_body'),('honeycomb_operations','receipt')) AS fields(table_name,column_name)
    LOOP
        EXECUTE format('UPDATE briefcase.%I SET %I=pg_temp.rekey_identity_json(%I) WHERE %I IS DISTINCT FROM pg_temp.rekey_identity_json(%I)',
                       item.table_name,item.column_name,item.column_name,item.column_name,item.column_name);
        GET DIAGNOSTICS changed=ROW_COUNT;
        INSERT INTO uuid_counts VALUES (item.table_name || '.' || item.column_name,changed);
    END LOOP;
    -- All pre-cutover dispatch is already drained by the maintenance guard.
    -- Never alter historical payload bytes, and prevent any later requeue.
    UPDATE briefcase.outbox_events event SET account_uuid_retired=true
     WHERE NOT account_uuid_retired AND (
         EXISTS (SELECT 1 FROM uuid_fresh input WHERE event.org_id=input.new_uuid)
         OR event.payload IS DISTINCT FROM pg_temp.rekey_identity_json(event.payload));
    GET DIAGNOSTICS changed=ROW_COUNT;
    INSERT INTO uuid_counts VALUES ('outbox_events.account_uuid_retired',changed);
    INSERT INTO briefcase.account_uuid_mappings(old_uuid,new_uuid,kind,mapping_sha256)
    SELECT input.old_uuid,input.new_uuid,input.kind,settings.digest FROM uuid_input input CROSS JOIN uuid_settings settings
    ON CONFLICT (old_uuid) DO NOTHING;
END $$;

SELECT pg_temp.link_maintenance_off();
INSERT INTO uuid_result SELECT jsonb_build_object(
    'status',CASE WHEN EXISTS(SELECT 1 FROM uuid_fresh) THEN 'applied' ELSE 'already_applied' END,
    'mapping_sha256',(SELECT digest FROM uuid_settings),
    'mapping_rows',(SELECT count(*) FROM uuid_input),
    'new_mapping_rows',(SELECT count(*) FROM uuid_fresh),
    'referenced_accounts',(SELECT count(*) FROM uuid_used),
    'unreferenced_accounts',(SELECT count(*) FROM uuid_fresh fresh WHERE NOT EXISTS(SELECT 1 FROM uuid_used used WHERE used.old_uuid=fresh.old_uuid)),
    'changed_references',COALESCE((SELECT jsonb_object_agg(reference,changed) FROM uuid_counts WHERE changed>0),'{}'::jsonb),
    'preserved','Resource UUIDs, IAM private keys, public IDs, S3 addresses, external IDs, ciphertext and historical identity-link ledgers');
