-- briefcase-migrate link-identities, part 5b: snapshot shares, usage,
-- aliases, the account cache and the identity links.

-- Each shared space ({org}-public, {org}-{tag}, apps/{app}/{org}-public) is
-- shared, read and write, inheriting, with exactly the accounts that could
-- reach it at the cutover: every active member, or every active member with
-- the tag. Members without an account get nothing (they are reported).
CREATE OR REPLACE FUNCTION pg_temp.link_snapshot_grants()
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
    space record;
    member record;
    granted integer := 0;
BEGIN
    FOR space IN SELECT * FROM link_spaces ORDER BY org, legacy_path LOOP
        FOR member IN
            SELECT member_row.actor_type, member_row.actor_id
              FROM briefcase.organization_members AS member_row
             WHERE member_row.org_id = space.org AND member_row.membership_status = 'active'
               AND pg_temp.link_is_mapped(member_row.actor_id)
               AND (space.audience = 'members' OR EXISTS (
                   SELECT 1 FROM briefcase.organization_member_tags AS member_tag
                     JOIN briefcase.organization_tags AS tag
                       ON tag.org_id = member_tag.org_id AND tag.tag_id = member_tag.tag_id
                      AND tag.lifecycle_status = 'active'
                    WHERE member_tag.org_id = member_row.org_id
                      AND member_tag.actor_type = member_row.actor_type
                      AND member_tag.actor_id = member_row.actor_id
                      AND member_tag.tag_id = space.tag_id))
             ORDER BY member_row.actor_id
        LOOP
            CONTINUE WHEN member.actor_id = space.drive;
            PERFORM pg_temp.link_grant(space.drive, space.folder, member.actor_type, member.actor_id,
                3::smallint, true, pg_temp.link_kind_of(space.drive), space.drive, NULL);
            granted := granted + 1;
        END LOOP;
    END LOOP;
    RETURN granted;
END;
$$;

-- Stored bytes are recomputed for every drive and organization the run
-- touched; an organization's limit overrides move to its owner's drive
-- unless that drive has its own.
CREATE OR REPLACE FUNCTION pg_temp.link_usage()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    target record;
    present record;
    legacy_limits record;
BEGIN
    FOR target IN
        SELECT DISTINCT drive AS org FROM link_moved
        UNION SELECT owner_uuid FROM link_orgs
        UNION SELECT org FROM link_legacy_orgs
    LOOP
        SELECT * INTO present FROM briefcase.organization_usage WHERE org_id = target.org;
        IF NOT FOUND THEN
            CONTINUE;
        END IF;
        IF NOT EXISTS (SELECT 1 FROM briefcase.identity_link_ledger
                        WHERE run_id = pg_temp.link_run_id() AND item = 'usage'
                          AND org_id = target.org) THEN
            PERFORM pg_temp.link_record('usage', target.org, jsonb_build_object('org_id', target.org),
                jsonb_build_object('stored_bytes', present.stored_bytes,
                                   'daily_upload_limit_bytes', present.daily_upload_limit_bytes,
                                   'storage_limit_bytes', present.storage_limit_bytes));
        END IF;
        UPDATE briefcase.organization_usage
           SET stored_bytes = (SELECT COALESCE(sum(version.size_bytes), 0)
                                 FROM briefcase.entry_versions AS version
                                WHERE version.org_id = target.org)
         WHERE org_id = target.org;
    END LOOP;
    FOR target IN SELECT * FROM link_orgs LOOP
        SELECT daily_upload_limit_bytes, storage_limit_bytes INTO legacy_limits
          FROM briefcase.organization_usage WHERE org_id = target.org;
        IF FOUND THEN
            UPDATE briefcase.organization_usage
               SET daily_upload_limit_bytes = COALESCE(daily_upload_limit_bytes,
                                                       legacy_limits.daily_upload_limit_bytes),
                   storage_limit_bytes = COALESCE(storage_limit_bytes, legacy_limits.storage_limit_bytes)
             WHERE org_id = target.owner_uuid;
        END IF;
    END LOOP;
END;
$$;

-- Addresses from before personal drives: every moved entry, every alias the
-- 2.0.0 identifier migration kept for them, and each structural folder whose
-- contents moved (its address leads to the folder that holds them now).
CREATE OR REPLACE FUNCTION pg_temp.link_aliases()
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
    written integer;
    more integer;
BEGIN
    INSERT INTO briefcase.legacy_path_aliases (legacy_org, legacy_path, drive, entry_id, run_id)
    SELECT moved_entry.legacy_org, moved_entry.legacy_path, moved_entry.drive,
           moved_entry.entry_id, pg_temp.link_run_id()
      FROM link_moved AS moved_entry
    ON CONFLICT (legacy_org, legacy_path) DO NOTHING;
    GET DIAGNOSTICS written = ROW_COUNT;
    INSERT INTO briefcase.legacy_path_aliases (legacy_org, legacy_path, drive, entry_id, run_id)
    SELECT moved_entry.legacy_org, alias.legacy_path, moved_entry.drive, moved_entry.entry_id,
           pg_temp.link_run_id()
      FROM briefcase.identifier_path_aliases AS alias
      JOIN link_moved AS moved_entry
        ON moved_entry.entry_id = alias.entry_id AND moved_entry.drive = alias.org_id
    ON CONFLICT (legacy_org, legacy_path) DO NOTHING;
    GET DIAGNOSTICS more = ROW_COUNT;
    written := written + more;
    INSERT INTO briefcase.legacy_path_aliases (legacy_org, legacy_path, drive, entry_id, run_id)
    SELECT container.org, container.legacy_path, container.drive, container.entry_id,
           pg_temp.link_run_id()
      FROM link_containers AS container
    ON CONFLICT (legacy_org, legacy_path) DO NOTHING;
    GET DIAGNOSTICS more = ROW_COUNT;
    RETURN written + more;
END;
$$;

-- The account cache learns every linked account Briefcase never saw (kind,
-- and the id the mapping gives), marked stale so the service looks it up on
-- first use; and the identity links are recorded.
CREATE OR REPLACE FUNCTION pg_temp.link_accounts()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
BEGIN
    FOR item IN SELECT * FROM link_map ORDER BY line LOOP
        IF NOT EXISTS (SELECT 1 FROM briefcase.accounts WHERE account_uuid = item.accounts_uuid) THEN
            INSERT INTO briefcase.accounts
                (account_uuid, kind, public_id, public_id_observed_at, refreshed_at)
            VALUES (item.accounts_uuid, item.kind, item.accounts_id,
                    CASE WHEN item.accounts_id IS NOT NULL THEN clock_timestamp() END,
                    'epoch'::timestamptz);
            PERFORM pg_temp.link_record('account', item.accounts_uuid,
                jsonb_build_object('account_uuid', item.accounts_uuid), NULL);
        END IF;
        INSERT INTO briefcase.identity_links
            (run_id, identity_kind, iam_public_id, iam_principal_id, accounts_uuid, source)
        VALUES (pg_temp.link_run_id(), item.kind, item.legacy_id, item.principal_id,
                item.accounts_uuid, 'mapping:' || item.line);
    END LOOP;
END;
$$;
