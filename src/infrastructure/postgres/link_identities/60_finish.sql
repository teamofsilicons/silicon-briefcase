-- briefcase-migrate link-identities, part 6: prove the result consistent and
-- describe it.

-- Paths and closures are recomputed from the parent links for every drive and
-- organization the run touched and compared with what is stored; one top per
-- drive, and `apps` at the top only as the apps folder.
CREATE OR REPLACE FUNCTION pg_temp.link_verify(p_orgs text[])
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    wrong bigint;
BEGIN
    WITH RECURSIVE computed AS (
        SELECT entry.org_id, entry.entry_id,
               CASE WHEN entry.system_kind = 'drive_root' THEN '.' ELSE entry.name END AS path,
               entry.system_kind = 'drive_root' AS at_root
          FROM briefcase.entries AS entry
         WHERE entry.org_id = ANY (p_orgs) AND entry.parent_id IS NULL
        UNION ALL
        SELECT child.org_id, child.entry_id,
               CASE WHEN parent.at_root THEN child.name ELSE parent.path || '/' || child.name END,
               false
          FROM briefcase.entries AS child
          JOIN computed AS parent ON parent.org_id = child.org_id AND parent.entry_id = child.parent_id
    )
    SELECT count(*) INTO wrong
      FROM briefcase.entries AS entry
      LEFT JOIN computed USING (org_id, entry_id)
     WHERE entry.org_id = ANY (p_orgs)
       AND computed.path IS DISTINCT FROM entry.path;
    IF wrong > 0 THEN
        RAISE EXCEPTION 'internal: % entries have a path that does not follow their folders; nothing was changed', wrong;
    END IF;

    WITH RECURSIVE chain AS (
        SELECT entry.org_id, entry.entry_id AS ancestor_id, entry.entry_id AS descendant_id,
               0 AS depth, entry.parent_id AS next_id
          FROM briefcase.entries AS entry
         WHERE entry.org_id = ANY (p_orgs)
        UNION ALL
        SELECT chain.org_id, parent.entry_id, chain.descendant_id, chain.depth + 1, parent.parent_id
          FROM chain
          JOIN briefcase.entries AS parent
            ON parent.org_id = chain.org_id AND parent.entry_id = chain.next_id
    ),
    stored AS (
        SELECT org_id, ancestor_id, descendant_id, depth
          FROM briefcase.entry_closure WHERE org_id = ANY (p_orgs)
    )
    SELECT count(*) INTO wrong FROM (
        (SELECT org_id, ancestor_id, descendant_id, depth FROM chain
         EXCEPT SELECT * FROM stored)
        UNION ALL
        (SELECT * FROM stored
         EXCEPT SELECT org_id, ancestor_id, descendant_id, depth FROM chain)
    ) AS difference;
    IF wrong > 0 THEN
        RAISE EXCEPTION 'internal: % folder links (closure rows) disagree with the folders; nothing was changed', wrong;
    END IF;

    SELECT count(*) INTO wrong
      FROM briefcase.organizations AS drive
     WHERE drive.org_id = ANY (p_orgs) AND drive.owner_account_uuid = drive.org_id
       AND (SELECT count(*) FROM briefcase.entries AS root
             WHERE root.org_id = drive.org_id AND root.system_kind = 'drive_root') <> 1;
    IF wrong > 0 THEN
        RAISE EXCEPTION 'internal: % drives do not have exactly one top folder; nothing was changed', wrong;
    END IF;
    SELECT count(*) INTO wrong
      FROM briefcase.entries AS entry
      JOIN briefcase.entries AS root
        ON root.org_id = entry.org_id AND root.entry_id = entry.parent_id
       AND root.system_kind = 'drive_root'
     WHERE entry.org_id = ANY (p_orgs) AND entry.name = 'apps' AND entry.deleted_at IS NULL
       AND entry.system_kind IS DISTINCT FROM 'apps_root';
    IF wrong > 0 THEN
        RAISE EXCEPTION 'internal: % drives have an ordinary folder named apps at the top; nothing was changed', wrong;
    END IF;
END;
$$;

-- The report printed to the operator and stored with the run.
CREATE OR REPLACE FUNCTION pg_temp.link_report(p_status text, p_previous uuid)
RETURNS jsonb
LANGUAGE sql
AS $$
    SELECT jsonb_build_object(
        'run_id', (SELECT run_id FROM link_settings),
        'status', p_status,
        'reverted_run', p_previous,
        'mapping', jsonb_build_object(
            'sha256', (SELECT mapping_sha256 FROM link_settings),
            'rows', (SELECT count(*) FROM link_input)),
        'linked', COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                       'line', line, 'kind', kind, 'legacy_id', legacy_id,
                       'accounts_uuid', accounts_uuid, 'accounts_id', accounts_id)
                   ORDER BY line)
              FROM link_map), '[]'::jsonb),
        'unmatched', COALESCE((
            SELECT jsonb_agg(jsonb_build_object('line', line, 'key', key, 'reason', reason)
                   ORDER BY line)
              FROM link_unmatched), '[]'::jsonb),
        'unlinked', COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                       'organization', member.org_id, 'kind', member.actor_type,
                       'legacy_id', member.actor_id, 'status', member.membership_status)
                   ORDER BY member.org_id, member.actor_id)
              FROM briefcase.organization_members AS member
              JOIN link_legacy_orgs ON link_legacy_orgs.org = member.org_id
             WHERE NOT pg_temp.link_is_mapped(member.actor_id)), '[]'::jsonb),
        'organizations', COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                'organization', organization.org,
                'owner', organization.owner_uuid,
                'moved_entries', (SELECT count(*) FROM link_moved WHERE legacy_org = organization.org),
                'link_shared_entries', (
                    SELECT count(*) FROM link_moved AS moved_entry
                      JOIN briefcase.entries AS entry
                        ON entry.org_id = moved_entry.drive AND entry.entry_id = moved_entry.entry_id
                     WHERE moved_entry.legacy_org = organization.org AND entry.link_public),
                'drives', (SELECT COALESCE(jsonb_agg(DISTINCT drive), '[]'::jsonb)
                             FROM link_moved WHERE legacy_org = organization.org),
                'spaces', COALESCE((
                    SELECT jsonb_agg(jsonb_build_object(
                               'legacy_path', space.legacy_path, 'drive', space.drive,
                               'path', entry.path,
                               'shared_with', (SELECT count(*) FROM briefcase.permission_grants AS share
                                                WHERE share.org_id = space.drive
                                                  AND share.entry_id = space.folder
                                                  AND share.revoked_at IS NULL))
                           ORDER BY space.legacy_path)
                      FROM link_spaces AS space
                      JOIN briefcase.entries AS entry
                        ON entry.org_id = space.drive AND entry.entry_id = space.folder
                     WHERE space.org = organization.org), '[]'::jsonb),
                'unclaimed', COALESCE((
                    SELECT jsonb_agg(jsonb_build_object(
                               'legacy_id', area.legacy_id, 'drive', area.drive, 'path', entry.path,
                               'entries', (SELECT count(*) - 1 FROM briefcase.entry_closure AS below
                                            WHERE below.org_id = area.drive
                                              AND below.ancestor_id = area.folder))
                           ORDER BY area.legacy_id)
                      FROM link_unclaimed AS area
                      JOIN briefcase.entries AS entry
                        ON entry.org_id = area.drive AND entry.entry_id = area.folder
                     WHERE area.org = organization.org), '[]'::jsonb),
                'renamed', COALESCE((
                    SELECT jsonb_agg(jsonb_build_object(
                               'legacy_path', renamed.legacy_path, 'drive', renamed.drive,
                               'name', renamed.name)
                           ORDER BY renamed.legacy_path)
                      FROM link_renamed AS renamed
                     WHERE renamed.org = organization.org), '[]'::jsonb))
                ORDER BY organization.org)
              FROM link_orgs AS organization), '[]'::jsonb),
        'notifications_moved', COALESCE((SELECT (value ->> 0)::bigint FROM link_state
                                          WHERE key = 'notifications'), 0),
        'tag_shares_expanded', COALESCE((SELECT (value ->> 0)::bigint FROM link_state
                                          WHERE key = 'tag_grants'), 0),
        'snapshot_shares', COALESCE((SELECT (value ->> 0)::bigint FROM link_state
                                      WHERE key = 'snapshot_grants'), 0),
        'aliases', COALESCE((SELECT (value ->> 0)::bigint FROM link_state WHERE key = 'aliases'), 0),
        'drives_created', (SELECT count(*) FROM briefcase.identity_link_ledger
                            WHERE run_id = (SELECT run_id FROM link_settings) AND item = 'drive'),
        'reverted_changes', COALESCE((SELECT (value ->> 0)::bigint FROM link_state
                                       WHERE key = 'reverted'), 0)
    )
$$;
