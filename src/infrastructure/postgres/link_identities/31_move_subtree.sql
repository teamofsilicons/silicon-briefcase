-- briefcase-migrate link-identities, part 3b: move one subtree.
--
-- Moves `p_root` and everything below it from tenant `p_from` into folder
-- `p_parent` of tenant `p_to`, named `p_name`. With `p_record` (a forward
-- move) the move is written to the ledger with every entry's previous path
-- and boundary, and recorded for the legacy aliases; without it (undoing a
-- run) the caller restores those facts from the ledger.

CREATE OR REPLACE FUNCTION pg_temp.link_move(
    p_root uuid, p_from text, p_to text, p_parent uuid, p_name text, p_record boolean
)
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
    old_path text;
    old_parent uuid;
    old_name text;
    new_parent_path text;
    new_path text;
    moved integer;
    previous jsonb;
    upload record;
BEGIN
    SELECT path, parent_id, name INTO old_path, old_parent, old_name
      FROM briefcase.entries WHERE org_id = p_from AND entry_id = p_root;
    IF old_path IS NULL THEN
        RAISE EXCEPTION 'internal: entry % is not in %', p_root, p_from;
    END IF;
    SELECT path INTO new_parent_path FROM briefcase.entries
     WHERE org_id = p_to AND entry_id = p_parent;
    IF new_parent_path IS NULL THEN
        RAISE EXCEPTION 'internal: folder % is not in %', p_parent, p_to;
    END IF;
    new_path := CASE WHEN new_parent_path = '.' THEN p_name ELSE new_parent_path || '/' || p_name END;

    TRUNCATE link_subtree;
    INSERT INTO link_subtree (entry_id, depth)
    SELECT closure.descendant_id, closure.depth FROM briefcase.entry_closure AS closure
     WHERE closure.org_id = p_from AND closure.ancestor_id = p_root;

    IF p_record THEN
        SELECT jsonb_agg(jsonb_build_object('id', entry.entry_id, 'path', entry.path,
                                            'root_type', entry.root_type, 'tag_id', entry.tag_id)
                         ORDER BY entry.path, entry.entry_id)
          INTO previous
          FROM briefcase.entries AS entry
          JOIN link_subtree USING (entry_id)
         WHERE entry.org_id = p_from;
        PERFORM pg_temp.link_record('move', p_to,
            jsonb_build_object('entry_id', p_root, 'from', p_from, 'to', p_to),
            jsonb_build_object('parent_id', old_parent, 'name', old_name, 'path', old_path,
                               'entries', previous));
        INSERT INTO link_moved (entry_id, legacy_org, legacy_path, drive)
        SELECT entry.entry_id, p_from, entry.path, p_to
          FROM briefcase.entries AS entry
          JOIN link_subtree USING (entry_id)
         WHERE entry.org_id = p_from;
    END IF;

    -- The closure: links to ancestors left behind go, links inside the
    -- subtree move, links to the new ancestors are added.
    DELETE FROM briefcase.entry_closure AS link
     USING link_subtree AS below
     WHERE link.org_id = p_from AND link.descendant_id = below.entry_id
       AND NOT EXISTS (SELECT 1 FROM link_subtree AS inside WHERE inside.entry_id = link.ancestor_id);
    UPDATE briefcase.entry_closure AS link SET org_id = p_to
      FROM link_subtree AS below
     WHERE link.org_id = p_from AND link.descendant_id = below.entry_id;
    INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
    SELECT p_to, above.ancestor_id, below.entry_id, above.depth + below.depth + 1
      FROM briefcase.entry_closure AS above
     CROSS JOIN link_subtree AS below
     WHERE above.org_id = p_to AND above.descendant_id = p_parent;

    UPDATE briefcase.entries AS entry
       SET org_id = p_to,
           path = new_path || substr(entry.path, length(old_path) + 1),
           root_type = 'private',
           tag_id = NULL,
           parent_id = CASE WHEN entry.entry_id = p_root THEN p_parent ELSE entry.parent_id END,
           name = CASE WHEN entry.entry_id = p_root THEN p_name ELSE entry.name END
      FROM link_subtree AS below
     WHERE entry.org_id = p_from AND entry.entry_id = below.entry_id;
    GET DIAGNOSTICS moved = ROW_COUNT;

    -- What belongs to the moved entries moves with them.
    UPDATE briefcase.entry_versions AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.permission_grants AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.tag_permission_grants AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.access_requests AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.search_documents AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.identifier_path_aliases AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;
    UPDATE briefcase.audit_events AS row_ SET org_id = p_to FROM link_subtree AS below
     WHERE row_.org_id = p_from AND row_.entry_id = below.entry_id;

    -- A finished upload follows the file it made; an unfinished one follows
    -- its folder. A folder left behind is replaced by the file's new folder.
    TRUNCATE link_multipart;
    INSERT INTO link_multipart (upload_id, completed, parent)
    SELECT upload_row.upload_id, upload_row.completed_entry_id, upload_row.parent_entry_id
      FROM briefcase.multipart_uploads AS upload_row
     WHERE upload_row.org_id = p_from
       AND (upload_row.completed_entry_id IN (SELECT entry_id FROM link_subtree)
            OR (upload_row.completed_entry_id IS NULL
                AND upload_row.parent_entry_id IN (SELECT entry_id FROM link_subtree)));
    UPDATE briefcase.multipart_uploads AS upload_row SET org_id = p_to
      FROM link_multipart AS chosen
     WHERE upload_row.org_id = p_from AND upload_row.upload_id = chosen.upload_id;
    UPDATE briefcase.multipart_parts AS part SET org_id = p_to
      FROM link_multipart AS chosen
     WHERE part.org_id = p_from AND part.upload_id = chosen.upload_id;
    IF p_record THEN
        FOR upload IN
            SELECT * FROM link_multipart
             WHERE parent IS NOT NULL AND parent NOT IN (SELECT entry_id FROM link_subtree)
        LOOP
            PERFORM pg_temp.link_record('multipart', p_to,
                jsonb_build_object('upload_id', upload.upload_id, 'move_root', p_root),
                jsonb_build_object('parent_entry_id', upload.parent));
            UPDATE briefcase.multipart_uploads
               SET parent_entry_id = (SELECT entry.parent_id FROM briefcase.entries AS entry
                                       WHERE entry.org_id = p_to AND entry.entry_id = upload.completed)
             WHERE org_id = p_to AND upload_id = upload.upload_id;
        END LOOP;
    END IF;
    RETURN moved;
END;
$$;
