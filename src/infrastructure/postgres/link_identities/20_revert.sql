-- briefcase-migrate link-identities, part 2: undo the applied run.
--
-- A corrected mapping is applied by running the command again: the run that
-- is in place is undone first, from its ledger, newest change first, and the
-- new mapping then starts from the state before any run. Undoing is refused
-- once Briefcase has been used in the drives that run touched (the cutover
-- happened); restore the backup taken before the run instead.

CREATE OR REPLACE FUNCTION pg_temp.link_revert(p_run uuid)
RETURNS bigint
LANGUAGE plpgsql
AS $$
DECLARE
    finished timestamptz;
    touched text[];
    item record;
    undone bigint := 0;
BEGIN
    SELECT finished_at INTO finished FROM briefcase.identity_link_runs WHERE run_id = p_run;
    SELECT COALESCE(array_agg(DISTINCT ledger.org_id), ARRAY[]::text[])
      INTO touched
      FROM briefcase.identity_link_ledger AS ledger
     WHERE ledger.run_id = p_run
       AND NOT EXISTS (SELECT 1 FROM link_legacy_orgs WHERE org = ledger.org_id);
    IF EXISTS (SELECT 1 FROM briefcase.audit_events
                WHERE org_id = ANY (touched) AND occurred_at > finished)
        OR EXISTS (SELECT 1 FROM briefcase.entries
                    WHERE org_id = ANY (touched) AND (created_at > finished OR updated_at > finished))
        OR EXISTS (SELECT 1 FROM briefcase.entry_versions
                    WHERE org_id = ANY (touched) AND created_at > finished)
        OR EXISTS (SELECT 1 FROM briefcase.permission_grants
                    WHERE org_id = ANY (touched) AND (created_at > finished OR revoked_at > finished))
        OR EXISTS (SELECT 1 FROM briefcase.notifications
                    WHERE org_id = ANY (touched) AND (created_at > finished OR read_at > finished))
    THEN
        RAISE EXCEPTION 'Briefcase has been used in the drives run % filled since it finished at %, so it can no longer be undone by running link-identities again; restore the database backup taken before that run, then run link-identities with the corrected mapping',
            p_run, finished;
    END IF;

    -- First every change and move is undone, newest first; rows the run
    -- created are removed only afterwards, because removing one (a tag copy,
    -- a participant) cascades to whatever still refers to it.
    FOR item IN
        SELECT ledger.* FROM briefcase.identity_link_ledger AS ledger
         WHERE ledger.run_id = p_run
           AND (ledger.item IN ('grant', 'tag_grant', 'notification', 'move')
                OR (ledger.item = 'usage' AND ledger.before IS NOT NULL))
         ORDER BY ledger.seq DESC
    LOOP
        CASE item.item
            WHEN 'grant' THEN
                IF item.before IS NULL THEN
                    DELETE FROM briefcase.permission_grants
                     WHERE org_id = item.org_id AND grant_id = (item.key ->> 'grant_id')::uuid;
                ELSE
                    UPDATE briefcase.permission_grants
                       SET access_mask = (item.before ->> 'access_mask')::smallint,
                           inherits_to_descendants = (item.before ->> 'inherits_to_descendants')::boolean
                     WHERE org_id = item.org_id AND grant_id = (item.key ->> 'grant_id')::uuid;
                END IF;
            WHEN 'tag_grant' THEN
                UPDATE briefcase.tag_permission_grants
                   SET revoked_at = (item.before ->> 'revoked_at')::timestamptz,
                       revoked_by_type = item.before ->> 'revoked_by_type',
                       revoked_by_id = item.before ->> 'revoked_by_id'
                 WHERE org_id = item.org_id AND grant_id = (item.key ->> 'grant_id')::uuid;
            WHEN 'notification' THEN
                UPDATE briefcase.notifications
                   SET org_id = item.before ->> 'org_id', details = item.before -> 'details'
                 WHERE org_id = item.org_id
                   AND notification_id = (item.key ->> 'notification_id')::uuid;
            WHEN 'usage' THEN
                UPDATE briefcase.organization_usage
                   SET stored_bytes = (item.before ->> 'stored_bytes')::bigint,
                       daily_upload_limit_bytes = (item.before ->> 'daily_upload_limit_bytes')::bigint,
                       storage_limit_bytes = (item.before ->> 'storage_limit_bytes')::bigint
                 WHERE org_id = item.org_id;
            WHEN 'move' THEN
                PERFORM pg_temp.link_unmove(p_run, item.org_id, item.key, item.before);
        END CASE;
        undone := undone + 1;
    END LOOP;

    FOR item IN
        SELECT ledger.* FROM briefcase.identity_link_ledger AS ledger
         WHERE ledger.run_id = p_run
           AND (ledger.item IN ('tag', 'storage_config', 'participant', 'folder', 'drive', 'account')
                OR (ledger.item = 'usage' AND ledger.before IS NULL))
         ORDER BY ledger.seq DESC
    LOOP
        CASE item.item
            WHEN 'usage' THEN
                DELETE FROM briefcase.organization_usage WHERE org_id = item.org_id;
            WHEN 'storage_config' THEN
                DELETE FROM briefcase.organization_storage_configs
                 WHERE org_id = item.org_id
                   AND storage_config_id = (item.key ->> 'storage_config_id')::uuid;
            WHEN 'tag' THEN
                IF EXISTS (SELECT 1 FROM briefcase.tag_permission_grants
                            WHERE org_id = item.org_id AND tag_id = item.key ->> 'tag_id') THEN
                    RAISE EXCEPTION 'cannot undo run %: a tag share in drive % still names tag %',
                        p_run, item.org_id, item.key ->> 'tag_id';
                END IF;
                DELETE FROM briefcase.organization_tags
                 WHERE org_id = item.org_id AND tag_id = item.key ->> 'tag_id';
            WHEN 'participant' THEN
                IF EXISTS (SELECT 1 FROM briefcase.notifications
                            WHERE org_id = item.org_id
                              AND recipient_type = item.key ->> 'actor_type'
                              AND recipient_id = item.key ->> 'actor_id') THEN
                    RAISE EXCEPTION 'cannot undo run %: a notification in drive % still belongs to %',
                        p_run, item.org_id, item.key ->> 'actor_id';
                END IF;
                DELETE FROM briefcase.organization_members
                 WHERE org_id = item.org_id
                   AND actor_type = item.key ->> 'actor_type'
                   AND actor_id = item.key ->> 'actor_id';
            WHEN 'folder' THEN
                IF EXISTS (SELECT 1 FROM briefcase.entries
                            WHERE org_id = item.org_id
                              AND parent_id = (item.key ->> 'entry_id')::uuid)
                    OR EXISTS (SELECT 1 FROM briefcase.permission_grants
                                WHERE org_id = item.org_id
                                  AND entry_id = (item.key ->> 'entry_id')::uuid) THEN
                    RAISE EXCEPTION 'cannot undo run %: folder % in drive % is no longer as the run left it',
                        p_run, item.key ->> 'entry_id', item.org_id;
                END IF;
                DELETE FROM briefcase.entries
                 WHERE org_id = item.org_id AND entry_id = (item.key ->> 'entry_id')::uuid;
            WHEN 'drive' THEN
                IF EXISTS (SELECT 1 FROM briefcase.entries WHERE org_id = item.org_id)
                    OR EXISTS (SELECT 1 FROM briefcase.notifications WHERE org_id = item.org_id)
                    OR EXISTS (SELECT 1 FROM briefcase.organization_members WHERE org_id = item.org_id)
                THEN
                    RAISE EXCEPTION 'cannot undo run %: drive % still holds something', p_run, item.org_id;
                END IF;
                DELETE FROM briefcase.organizations WHERE org_id = item.org_id;
            WHEN 'account' THEN
                DELETE FROM briefcase.accounts WHERE account_uuid = item.key ->> 'account_uuid';
        END CASE;
        undone := undone + 1;
    END LOOP;

    PERFORM pg_temp.link_rekey(ARRAY(SELECT org FROM link_legacy_orgs), p_run, true);
    DELETE FROM briefcase.legacy_path_aliases WHERE run_id = p_run;
    UPDATE briefcase.identity_link_runs
       SET status = 'reverted', reverted_at = clock_timestamp(), reverted_by_run = pg_temp.link_run_id()
     WHERE run_id = p_run;
    RETURN undone;
END;
$$;

-- Puts one moved subtree back where it was, exactly: the organization, the
-- folder, the name, each entry's path and boundary, and the folder each
-- finished upload pointed at.
CREATE OR REPLACE FUNCTION pg_temp.link_unmove(p_run uuid, p_drive text, p_key jsonb, p_before jsonb)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    legacy_org text := p_key ->> 'from';
    root uuid := (p_key ->> 'entry_id')::uuid;
BEGIN
    PERFORM pg_temp.link_move(root, p_drive, legacy_org, (p_before ->> 'parent_id')::uuid,
                              p_before ->> 'name', false);
    UPDATE briefcase.entries AS entry
       SET path = legacy.path, root_type = legacy.root_type, tag_id = legacy.tag_id
      FROM jsonb_to_recordset(p_before -> 'entries')
               AS legacy (id uuid, path text, root_type text, tag_id text)
     WHERE entry.org_id = legacy_org AND entry.entry_id = legacy.id;
    UPDATE briefcase.multipart_uploads AS upload
       SET parent_entry_id = (ledger.before ->> 'parent_entry_id')::uuid
      FROM briefcase.identity_link_ledger AS ledger
     WHERE ledger.run_id = p_run AND ledger.item = 'multipart'
       AND ledger.key ->> 'move_root' = root::text
       AND upload.org_id = legacy_org
       AND upload.upload_id = (ledger.key ->> 'upload_id')::uuid;
END;
$$;

-- Re-keys every account column in `p_orgs`: legacy ids to Silicon Accounts
-- uuids from this run's mapping, or (`p_reverse`) back from a run's links.
CREATE OR REPLACE FUNCTION pg_temp.link_rekey(p_orgs text[], p_run uuid, p_reverse boolean)
RETURNS bigint
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
    affected bigint;
    changed bigint := 0;
BEGIN
    FOR item IN SELECT * FROM link_actor_columns ORDER BY table_name, id_column LOOP
        IF p_reverse THEN
            EXECUTE format(
                'UPDATE briefcase.%I AS target SET %I = link.iam_public_id '
                'FROM briefcase.identity_links AS link '
                'WHERE link.run_id = $2 AND target.org_id = ANY ($1) '
                'AND target.%I = link.identity_kind AND target.%I = link.accounts_uuid',
                item.table_name, item.id_column, item.kind_column, item.id_column)
            USING p_orgs, p_run;
        ELSE
            EXECUTE format(
                'UPDATE briefcase.%I AS target SET %I = map.accounts_uuid '
                'FROM link_map AS map '
                'WHERE target.org_id = ANY ($1) '
                'AND target.%I = map.kind AND target.%I = map.legacy_id',
                item.table_name, item.id_column, item.kind_column, item.id_column)
            USING p_orgs;
        END IF;
        GET DIAGNOSTICS affected = ROW_COUNT;
        changed := changed + affected;
    END LOOP;
    RETURN changed;
END;
$$;
