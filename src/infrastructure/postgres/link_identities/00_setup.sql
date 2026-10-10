-- briefcase-migrate link-identities, part 0: session, ledger, maintenance mode.
--
-- The command runs this program in one transaction after filling two temp
-- tables:
--   link_input(line integer, key text, accounts_uuid text, kind text, accounts_id text)
--   link_settings(run_id uuid, mapping_sha256 text, mapping jsonb)
-- It reads the report from link_result(report jsonb) and then commits, or
-- rolls back for a dry run. Any refusal is a RAISE EXCEPTION whose message
-- names exactly what to fix; nothing is changed then.

SET LOCAL lock_timeout = '15s';
SET LOCAL statement_timeout = '30min';

DROP SEQUENCE IF EXISTS pg_temp.link_seq;
CREATE TEMP SEQUENCE link_seq;
CREATE TEMP TABLE link_result (report jsonb NOT NULL) ON COMMIT DROP;
CREATE TEMP TABLE link_state (key text PRIMARY KEY, value jsonb) ON COMMIT DROP;

-- One change of this run, so a later run can undo it (see
-- briefcase.identity_link_ledger).
CREATE OR REPLACE FUNCTION pg_temp.link_record(p_item text, p_org text, p_key jsonb, p_before jsonb)
RETURNS void
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO briefcase.identity_link_ledger (run_id, seq, item, org_id, key, before)
    SELECT settings.run_id, nextval('pg_temp.link_seq'), p_item, p_org, p_key, p_before
      FROM link_settings AS settings;
END;
$$;

CREATE OR REPLACE FUNCTION pg_temp.link_run_id()
RETURNS uuid
LANGUAGE sql
STABLE
AS $$ SELECT run_id FROM link_settings $$;

-- Exclusive locks, row security off, every user trigger off and every foreign
-- key deferred: the program maintains paths, closures and participants
-- itself, and nothing else can see the intermediate state. The original
-- settings are restored by pg_temp.link_maintenance_off().
CREATE OR REPLACE FUNCTION pg_temp.link_maintenance_on()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
BEGIN
    IF to_regclass('briefcase.identity_link_runs') IS NULL THEN
        RAISE EXCEPTION 'migration 0050 is missing: run briefcase-migrate (without arguments) first';
    END IF;
    CREATE TEMP TABLE link_tables ON COMMIT DROP AS
        SELECT class.oid, class.oid::regclass AS rel, class.relrowsecurity, class.relforcerowsecurity
          FROM pg_catalog.pg_class AS class
         WHERE class.relnamespace = 'briefcase'::regnamespace AND class.relkind = 'r';
    FOR item IN SELECT * FROM link_tables ORDER BY oid LOOP
        EXECUTE format('LOCK TABLE %s IN ACCESS EXCLUSIVE MODE', item.rel);
    END LOOP;
    CREATE TEMP TABLE link_triggers ON COMMIT DROP AS
        SELECT trigger.tgrelid::regclass AS rel, trigger.tgname, trigger.tgenabled
          FROM pg_catalog.pg_trigger AS trigger
          JOIN link_tables ON link_tables.oid = trigger.tgrelid
         WHERE NOT trigger.tgisinternal AND trigger.tgenabled <> 'D';
    FOR item IN SELECT * FROM link_triggers LOOP
        EXECUTE format('ALTER TABLE %s DISABLE TRIGGER %I', item.rel, item.tgname);
    END LOOP;
    FOR item IN SELECT * FROM link_tables ORDER BY oid LOOP
        EXECUTE format('ALTER TABLE %s DISABLE ROW LEVEL SECURITY', item.rel);
    END LOOP;
    CREATE TEMP TABLE link_constraints ON COMMIT DROP AS
        SELECT constraint_row.conrelid::regclass AS rel, constraint_row.conname,
               constraint_row.condeferrable, constraint_row.condeferred
          FROM pg_catalog.pg_constraint AS constraint_row
         WHERE constraint_row.contype = 'f'
           AND constraint_row.connamespace = 'briefcase'::regnamespace;
    FOR item IN SELECT * FROM link_constraints LOOP
        EXECUTE format('ALTER TABLE %s ALTER CONSTRAINT %I DEFERRABLE INITIALLY DEFERRED',
                       item.rel, item.conname);
    END LOOP;
    SET CONSTRAINTS ALL DEFERRED;

    IF EXISTS (SELECT 1 FROM briefcase.idempotency_records WHERE status = 'in_progress')
        OR EXISTS (SELECT 1 FROM briefcase.testing_environment_idempotency WHERE status = 'in_progress')
        OR EXISTS (SELECT 1 FROM briefcase.delegated_uploads
                    WHERE status IN ('reserved', 'receiving', 'staged', 'cleanup_pending'))
        OR EXISTS (SELECT 1 FROM briefcase.multipart_uploads
                    WHERE status IN ('initiated', 'uploading', 'completing'))
        OR EXISTS (SELECT 1 FROM briefcase.outbox_events WHERE status IN ('pending', 'processing'))
    THEN
        RAISE EXCEPTION 'work is still in flight (writes, uploads, cleanups or emails): stop the API, let the worker drain, then run link-identities again';
    END IF;
END;
$$;

-- Checks every foreign key now, then restores the triggers, constraint
-- timing and row security exactly as they were.
CREATE OR REPLACE FUNCTION pg_temp.link_maintenance_off()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
BEGIN
    SET CONSTRAINTS ALL IMMEDIATE;
    FOR item IN SELECT * FROM link_constraints LOOP
        EXECUTE format('ALTER TABLE %s ALTER CONSTRAINT %I %s INITIALLY %s',
                       item.rel, item.conname,
                       CASE WHEN item.condeferrable THEN 'DEFERRABLE' ELSE 'NOT DEFERRABLE' END,
                       CASE WHEN item.condeferred THEN 'DEFERRED' ELSE 'IMMEDIATE' END);
    END LOOP;
    FOR item IN SELECT * FROM link_triggers LOOP
        EXECUTE format('ALTER TABLE %s %s TRIGGER %I', item.rel,
                       CASE item.tgenabled
                           WHEN 'A' THEN 'ENABLE ALWAYS'
                           WHEN 'R' THEN 'ENABLE REPLICA'
                           ELSE 'ENABLE'
                       END,
                       item.tgname);
    END LOOP;
    FOR item IN SELECT * FROM link_tables ORDER BY oid LOOP
        EXECUTE format('ALTER TABLE %s %s ROW LEVEL SECURITY', item.rel,
                       CASE WHEN item.relrowsecurity THEN 'ENABLE' ELSE 'DISABLE' END);
        EXECUTE format('ALTER TABLE %s %s ROW LEVEL SECURITY', item.rel,
                       CASE WHEN item.relforcerowsecurity THEN 'FORCE' ELSE 'NO FORCE' END);
    END LOOP;
END;
$$;

-- Every (kind, id) pair of columns naming an account, with the tables that
-- store them. A schema change that adds another must be added here: the
-- program refuses to run while an unlisted pair exists.
CREATE TEMP TABLE link_actor_columns (
    table_name text NOT NULL,
    kind_column text NOT NULL,
    id_column text NOT NULL,
    PRIMARY KEY (table_name, id_column)
) ON COMMIT DROP;
INSERT INTO link_actor_columns VALUES
    ('organization_members', 'actor_type', 'actor_id'),
    ('organization_member_tags', 'actor_type', 'actor_id'),
    ('entries', 'owner_type', 'owner_id'),
    ('entries', 'created_by_type', 'created_by_id'),
    ('entries', 'updated_by_type', 'updated_by_id'),
    ('entry_versions', 'created_by_type', 'created_by_id'),
    ('organization_storage_configs', 'created_by_type', 'created_by_id'),
    ('permission_grants', 'principal_type', 'principal_id'),
    ('permission_grants', 'granted_by_type', 'granted_by_id'),
    ('permission_grants', 'revoked_by_type', 'revoked_by_id'),
    ('tag_permission_grants', 'granted_by_type', 'granted_by_id'),
    ('tag_permission_grants', 'revoked_by_type', 'revoked_by_id'),
    ('access_requests', 'requested_by_type', 'requested_by_id'),
    ('access_requests', 'decided_by_type', 'decided_by_id'),
    ('idempotency_records', 'actor_type', 'actor_id'),
    ('audit_events', 'actor_type', 'actor_id'),
    ('notifications', 'recipient_type', 'recipient_id'),
    ('notifications', 'actor_type', 'actor_id'),
    ('member_contacts', 'actor_type', 'actor_id'),
    ('multipart_uploads', 'owner_type', 'owner_id'),
    ('delegated_uploads', 'actor_type', 'actor_id'),
    ('bug_reports', 'actor_type', 'actor_id'),
    ('testing_environments', 'created_by_type', 'created_by_id');

-- Pairs that look alike but do not name accounts.
CREATE TEMP TABLE link_other_columns (
    table_name text NOT NULL,
    id_column text NOT NULL,
    PRIMARY KEY (table_name, id_column)
) ON COMMIT DROP;
INSERT INTO link_other_columns VALUES
    ('entries', 'entry_id'),
    ('webhook_receipts', 'aggregate_id'),
    ('webhook_receipts', 'event_id'),
    ('outbox_events', 'aggregate_id'),
    ('accounts_webhook_receipts', 'event_id'),
    ('testing_environment_idempotency', 'authority_id'),
    ('effective_permission_grants', 'principal_id'),
    ('effective_permission_grants', 'granted_by_id'),
    ('effective_permission_grants', 'revoked_by_id');

CREATE OR REPLACE FUNCTION pg_temp.link_check_actor_columns()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    unknown text;
BEGIN
    SELECT string_agg(found.table_name || '.' || found.column_name, ', ' ORDER BY 1)
      INTO unknown
      FROM information_schema.columns AS found
     WHERE found.table_schema = 'briefcase'
       AND found.column_name LIKE '%\_id' ESCAPE '\'
       AND EXISTS (
           SELECT 1 FROM information_schema.columns AS kind
            WHERE kind.table_schema = found.table_schema AND kind.table_name = found.table_name
              AND kind.column_name = replace(found.column_name, '_id', '_type'))
       AND NOT EXISTS (SELECT 1 FROM link_actor_columns AS listed
                        WHERE listed.table_name = found.table_name
                          AND listed.id_column = found.column_name)
       AND NOT EXISTS (SELECT 1 FROM link_other_columns AS other
                        WHERE other.table_name = found.table_name
                          AND other.id_column = found.column_name);
    IF unknown IS NOT NULL THEN
        RAISE EXCEPTION 'link-identities does not know these account columns: %; add them to link_actor_columns before running it', unknown;
    END IF;
END;
$$;
