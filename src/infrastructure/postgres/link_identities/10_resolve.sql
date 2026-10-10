-- briefcase-migrate link-identities, part 1: resolve the operator's mapping.
--
-- Each mapping row names a legacy identity, either by the public id stored
-- in pre-migration rows (`c:saket`, `si:scout`) or by its IAM principal UUID
-- (matched through iam_identity_bindings, organization_members.principal_id
-- and delegated_uploads.iam_principal_id), or names the account whose drive
-- receives an organization's shared spaces (`org:tos`). Rows that match
-- nothing are reported, never guessed at.

CREATE TEMP TABLE link_map (
    kind text NOT NULL,
    legacy_id text NOT NULL,
    accounts_uuid text NOT NULL,
    accounts_id text,
    principal_id uuid,
    line integer NOT NULL,
    PRIMARY KEY (kind, legacy_id)
) ON COMMIT DROP;
CREATE TEMP TABLE link_owner_override (
    org text PRIMARY KEY,
    accounts_uuid text NOT NULL,
    line integer NOT NULL
) ON COMMIT DROP;
CREATE TEMP TABLE link_unmatched (
    line integer NOT NULL,
    key text NOT NULL,
    reason text NOT NULL
) ON COMMIT DROP;
CREATE TEMP TABLE link_legacy_orgs ON COMMIT DROP AS
    SELECT organization.org_id AS org
      FROM briefcase.organizations AS organization
     WHERE organization.owner_account_uuid IS NULL
       AND organization.testing_environment_id IS NULL;

CREATE OR REPLACE FUNCTION pg_temp.link_add_mapping(
    p_line integer, p_kind text, p_legacy text, p_uuid text, p_accounts_id text, p_principal uuid
)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    existing record;
BEGIN
    SELECT * INTO existing FROM link_map WHERE kind = p_kind AND legacy_id = p_legacy;
    IF FOUND THEN
        IF existing.accounts_uuid <> p_uuid THEN
            RAISE EXCEPTION 'line %: % is linked to % here but to % on line %; one legacy identity becomes exactly one account',
                p_line, p_legacy, p_uuid, existing.accounts_uuid, existing.line;
        END IF;
        UPDATE link_map
           SET principal_id = COALESCE(link_map.principal_id, p_principal),
               accounts_id = COALESCE(link_map.accounts_id, p_accounts_id)
         WHERE kind = p_kind AND legacy_id = p_legacy;
        RETURN;
    END IF;
    SELECT * INTO existing FROM link_map WHERE accounts_uuid = p_uuid;
    IF FOUND THEN
        RAISE EXCEPTION 'line %: account % is already linked to % on line %; one account receives exactly one legacy identity',
            p_line, p_uuid, existing.legacy_id, existing.line;
    END IF;
    INSERT INTO link_map (kind, legacy_id, accounts_uuid, accounts_id, principal_id, line)
    VALUES (p_kind, p_legacy, p_uuid, p_accounts_id, p_principal, p_line);
END;
$$;

CREATE OR REPLACE FUNCTION pg_temp.link_resolve()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
    found_identities text[];
    legacy_kind text;
BEGIN
    FOR item IN SELECT * FROM link_input ORDER BY line LOOP
        IF item.key LIKE 'org:%' THEN
            IF NOT EXISTS (SELECT 1 FROM link_legacy_orgs WHERE org = substr(item.key, 5)) THEN
                INSERT INTO link_unmatched
                VALUES (item.line, item.key, 'no organization from before personal drives has this key');
            ELSIF EXISTS (SELECT 1 FROM link_owner_override WHERE org = substr(item.key, 5)) THEN
                RAISE EXCEPTION 'line %: % is named twice; name one account for each organization', item.line, item.key;
            ELSE
                INSERT INTO link_owner_override VALUES (substr(item.key, 5), item.accounts_uuid, item.line);
            END IF;
            CONTINUE;
        END IF;
        IF item.key ~ '^(c|si):' THEN
            legacy_kind := CASE WHEN item.key LIKE 'c:%' THEN 'carbon' ELSE 'silicon' END;
            IF NOT EXISTS (
                SELECT 1 FROM briefcase.organization_members AS member
                  JOIN link_legacy_orgs ON link_legacy_orgs.org = member.org_id
                 WHERE member.actor_type = legacy_kind AND member.actor_id = item.key
                UNION ALL
                SELECT 1 FROM briefcase.iam_identity_bindings AS binding
                 WHERE binding.environment_id = '00000000-0000-0000-0000-000000000000'
                   AND binding.identity_kind = legacy_kind AND binding.public_id = item.key
                UNION ALL
                SELECT 1 FROM briefcase.entries AS entry
                  JOIN link_legacy_orgs ON link_legacy_orgs.org = entry.org_id
                 WHERE (entry.owner_type = legacy_kind AND entry.owner_id = item.key)
                    OR (entry.created_by_type = legacy_kind AND entry.created_by_id = item.key)
            ) THEN
                INSERT INTO link_unmatched
                VALUES (item.line, item.key, 'no member, binding or file from before personal drives names this id');
                CONTINUE;
            END IF;
            IF item.kind IS NOT NULL AND item.kind <> legacy_kind THEN
                RAISE EXCEPTION 'line %: % is a % id but the line says %', item.line, item.key, legacy_kind, item.kind;
            END IF;
            PERFORM pg_temp.link_add_mapping(item.line, legacy_kind, item.key, item.accounts_uuid,
                                             item.accounts_id, NULL);
            CONTINUE;
        END IF;
        -- An IAM principal UUID: find the one legacy identity bound to it.
        SELECT array_agg(DISTINCT pair.kind || ' ' || pair.public_id ORDER BY pair.kind || ' ' || pair.public_id)
          INTO found_identities
          FROM (
              SELECT binding.identity_kind AS kind, binding.public_id
                FROM briefcase.iam_identity_bindings AS binding
               WHERE binding.environment_id = '00000000-0000-0000-0000-000000000000'
                 AND binding.identity_kind IN ('carbon', 'silicon')
                 AND binding.local_id = item.key::uuid
              UNION
              SELECT member.actor_type, member.actor_id
                FROM briefcase.organization_members AS member
                JOIN link_legacy_orgs ON link_legacy_orgs.org = member.org_id
               WHERE member.principal_id = item.key::uuid
              UNION
              SELECT upload.actor_type, upload.actor_id
                FROM briefcase.delegated_uploads AS upload
                JOIN link_legacy_orgs ON link_legacy_orgs.org = upload.org_id
               WHERE upload.iam_principal_id = item.key::uuid
          ) AS pair;
        IF found_identities IS NULL THEN
            INSERT INTO link_unmatched
            VALUES (item.line, item.key, 'no legacy identity is bound to this IAM principal id');
            CONTINUE;
        END IF;
        IF cardinality(found_identities) > 1 THEN
            RAISE EXCEPTION 'line %: IAM principal % is bound to several legacy identities (%); name each by its c:/si: id instead',
                item.line, item.key, array_to_string(found_identities, ', ');
        END IF;
        legacy_kind := split_part(found_identities[1], ' ', 1);
        IF item.kind IS NOT NULL AND item.kind <> legacy_kind THEN
            RAISE EXCEPTION 'line %: IAM principal % is a % but the line says %', item.line, item.key, legacy_kind, item.kind;
        END IF;
        PERFORM pg_temp.link_add_mapping(item.line, legacy_kind, split_part(found_identities[1], ' ', 2),
                                         item.accounts_uuid, item.accounts_id, item.key::uuid);
    END LOOP;

    FOR item IN SELECT * FROM link_map ORDER BY line LOOP
        IF item.accounts_id IS NOT NULL
            AND (item.accounts_id LIKE 'c:%') <> (item.kind = 'carbon')
        THEN
            RAISE EXCEPTION 'line %: % is a % but its account id % is not', item.line, item.legacy_id, item.kind, item.accounts_id;
        END IF;
        IF EXISTS (SELECT 1 FROM link_legacy_orgs WHERE org = item.accounts_uuid) THEN
            RAISE EXCEPTION 'line %: account uuid % is also the key of an organization from before personal drives, so it cannot become a drive; resolve that organization first',
                item.line, item.accounts_uuid;
        END IF;
        IF EXISTS (SELECT 1 FROM briefcase.accounts AS account
                    WHERE account.account_uuid = item.accounts_uuid AND account.kind <> item.kind) THEN
            RAISE EXCEPTION 'line %: Silicon Accounts records % as another kind of account than %', item.line, item.accounts_uuid, item.legacy_id;
        END IF;
        UPDATE link_map
           SET principal_id = COALESCE(link_map.principal_id, (
                   SELECT binding.local_id FROM briefcase.iam_identity_bindings AS binding
                    WHERE binding.environment_id = '00000000-0000-0000-0000-000000000000'
                      AND binding.identity_kind = item.kind AND binding.public_id = item.legacy_id))
         WHERE kind = item.kind AND legacy_id = item.legacy_id;
    END LOOP;
    FOR item IN SELECT * FROM link_owner_override LOOP
        IF EXISTS (SELECT 1 FROM link_legacy_orgs WHERE org = item.accounts_uuid) THEN
            RAISE EXCEPTION 'line %: account uuid % is also the key of an organization from before personal drives', item.line, item.accounts_uuid;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM link_map WHERE accounts_uuid = item.accounts_uuid)
            AND NOT EXISTS (SELECT 1 FROM briefcase.accounts WHERE account_uuid = item.accounts_uuid)
        THEN
            RAISE EXCEPTION 'line %: % receives org:% but is neither linked to a legacy identity nor known to Briefcase; link it (c:… or si:…,%) too',
                item.line, item.accounts_uuid, item.org, item.accounts_uuid;
        END IF;
    END LOOP;
END;
$$;
