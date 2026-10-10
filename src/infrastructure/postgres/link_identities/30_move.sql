-- briefcase-migrate link-identities, part 3: drives, folders and moving a
-- subtree between tenants.
--
-- Triggers are off while the program runs, so these helpers maintain what
-- the triggers normally do: materialized paths, the closure table, and the
-- rows that belong to an entry (versions, shares, search documents, logs,
-- uploads, aliases) moving with it.

CREATE TEMP TABLE link_folders (
    key text PRIMARY KEY,
    drive text NOT NULL,
    entry_id uuid NOT NULL
) ON COMMIT DROP;
CREATE TEMP TABLE link_subtree (entry_id uuid PRIMARY KEY, depth integer NOT NULL) ON COMMIT DROP;
CREATE TEMP TABLE link_multipart (
    upload_id uuid PRIMARY KEY,
    completed uuid,
    parent uuid
) ON COMMIT DROP;
CREATE TEMP TABLE link_moved (
    entry_id uuid PRIMARY KEY,
    legacy_org text NOT NULL,
    legacy_path text NOT NULL,
    drive text NOT NULL
) ON COMMIT DROP;

-- The kind (carbon or silicon) of an account this run knows.
CREATE OR REPLACE FUNCTION pg_temp.link_kind_of(p_uuid text)
RETURNS text
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
    found_kind text;
BEGIN
    SELECT kind INTO found_kind FROM link_map WHERE accounts_uuid = p_uuid;
    IF found_kind IS NULL THEN
        SELECT kind INTO found_kind FROM briefcase.accounts WHERE account_uuid = p_uuid;
    END IF;
    RETURN found_kind;
END;
$$;

-- Creates a folder (or a drive's root when `p_parent` is NULL) with its path
-- and closure rows.
CREATE OR REPLACE FUNCTION pg_temp.link_new_folder(
    p_drive text, p_parent uuid, p_name text, p_system_kind text,
    p_owner_kind text, p_owner_id text, p_app text
)
RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    created uuid := briefcase.new_uuid_v7();
    parent_path text;
BEGIN
    IF p_parent IS NOT NULL THEN
        SELECT path INTO parent_path FROM briefcase.entries
         WHERE org_id = p_drive AND entry_id = p_parent;
        IF parent_path IS NULL THEN
            RAISE EXCEPTION 'internal: folder % is not in drive %', p_parent, p_drive;
        END IF;
    END IF;
    INSERT INTO briefcase.entries (
        org_id, entry_id, parent_id, entry_type, name, root_type, system_kind,
        owner_type, owner_id, origin_app_id, created_by_type, created_by_id,
        updated_by_type, updated_by_id, path
    ) VALUES (
        p_drive, created, p_parent, 'folder', p_name, 'private', p_system_kind,
        p_owner_kind, p_owner_id, p_app, p_owner_kind, p_owner_id, p_owner_kind, p_owner_id,
        CASE
            WHEN p_parent IS NULL THEN '.'
            WHEN parent_path = '.' THEN p_name
            ELSE parent_path || '/' || p_name
        END
    );
    INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
    VALUES (p_drive, created, created, 0);
    INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
    SELECT p_drive, above.ancestor_id, created, above.depth + 1
      FROM briefcase.entry_closure AS above
     WHERE above.org_id = p_drive AND above.descendant_id = p_parent;
    PERFORM pg_temp.link_record('folder', p_drive, jsonb_build_object('entry_id', created), NULL);
    RETURN created;
END;
$$;

-- A name for something placed in `p_parent` that no active sibling has:
-- the name itself, else `name (from org)`, `name (from org 2)`, … The top
-- of a drive also keeps `apps` for apps.
CREATE OR REPLACE FUNCTION pg_temp.link_free_name(p_drive text, p_parent uuid, p_name text, p_org text)
RETURNS text
LANGUAGE plpgsql
AS $$
DECLARE
    at_top boolean;
    candidate text := p_name;
    suffix text;
    prefix text;
    attempt integer := 1;
BEGIN
    SELECT system_kind = 'drive_root' INTO at_top FROM briefcase.entries
     WHERE org_id = p_drive AND entry_id = p_parent;
    LOOP
        IF NOT (COALESCE(at_top, false) AND candidate = 'apps')
            AND NOT EXISTS (
                SELECT 1 FROM briefcase.entries
                 WHERE org_id = p_drive AND parent_id = p_parent AND name = candidate
                   AND deleted_at IS NULL)
        THEN
            RETURN candidate;
        END IF;
        suffix := ' (from ' || p_org || CASE WHEN attempt > 1 THEN ' ' || attempt ELSE '' END || ')';
        prefix := p_name;
        WHILE octet_length(prefix || suffix) > 255 LOOP
            prefix := left(prefix, length(prefix) - 1);
        END LOOP;
        candidate := rtrim(prefix) || suffix;
        attempt := attempt + 1;
        IF attempt > 1000 THEN
            RAISE EXCEPTION 'internal: no free name for % in drive %', p_name, p_drive;
        END IF;
    END LOOP;
END;
$$;

-- An account's drive, created when it never had one: the drive row, the
-- owner as participant, the root folder and the usage row. Returns the root.
CREATE OR REPLACE FUNCTION pg_temp.link_drive(p_uuid text)
RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    owner_kind text := pg_temp.link_kind_of(p_uuid);
    root uuid;
BEGIN
    SELECT entry_id INTO root FROM link_folders WHERE key = 'root:' || p_uuid;
    IF root IS NOT NULL THEN
        RETURN root;
    END IF;
    IF owner_kind IS NULL THEN
        RAISE EXCEPTION 'internal: account % is not known to this run', p_uuid;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM briefcase.organizations WHERE org_id = p_uuid) THEN
        INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status, owner_account_uuid)
        VALUES (p_uuid, 0, 'active', p_uuid);
        PERFORM pg_temp.link_record('drive', p_uuid, jsonb_build_object('org_id', p_uuid), NULL);
    ELSIF NOT EXISTS (SELECT 1 FROM briefcase.organizations
                       WHERE org_id = p_uuid AND owner_account_uuid = p_uuid) THEN
        RAISE EXCEPTION 'account uuid % is the key of an organization from before personal drives', p_uuid;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM briefcase.organization_members
                    WHERE org_id = p_uuid AND actor_type = owner_kind AND actor_id = p_uuid) THEN
        INSERT INTO briefcase.organization_members
            (org_id, actor_type, actor_id, org_role, membership_status, iam_version)
        VALUES (p_uuid, owner_kind, p_uuid, 'owner', 'active', 0);
        PERFORM pg_temp.link_record('participant', p_uuid,
            jsonb_build_object('actor_type', owner_kind, 'actor_id', p_uuid), NULL);
    END IF;
    SELECT entry_id INTO root FROM briefcase.entries
     WHERE org_id = p_uuid AND system_kind = 'drive_root';
    IF root IS NULL THEN
        root := pg_temp.link_new_folder(p_uuid, NULL, p_uuid, 'drive_root', owner_kind, p_uuid, NULL);
    END IF;
    IF NOT EXISTS (SELECT 1 FROM briefcase.organization_usage WHERE org_id = p_uuid) THEN
        INSERT INTO briefcase.organization_usage (org_id, daily_window)
        VALUES (p_uuid, (clock_timestamp() AT TIME ZONE 'UTC')::date);
        PERFORM pg_temp.link_record('usage', p_uuid, jsonb_build_object('org_id', p_uuid), NULL);
    END IF;
    INSERT INTO link_folders VALUES ('root:' || p_uuid, p_uuid, root);
    RETURN root;
END;
$$;

-- A folder this run makes once (a shared space, an unclaimed area), named
-- so it collides with nothing already there, owned by the drive's owner.
CREATE OR REPLACE FUNCTION pg_temp.link_folder(
    p_key text, p_drive text, p_parent uuid, p_name text, p_org text,
    p_link_public boolean DEFAULT false, p_link_expires_at timestamptz DEFAULT NULL
)
RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    existing uuid;
    created uuid;
    owner_kind text := pg_temp.link_kind_of(p_drive);
BEGIN
    SELECT entry_id INTO existing FROM link_folders WHERE key = p_key;
    IF existing IS NOT NULL THEN
        RETURN existing;
    END IF;
    created := pg_temp.link_new_folder(p_drive, p_parent,
                                       pg_temp.link_free_name(p_drive, p_parent, p_name, p_org),
                                       NULL, owner_kind, p_drive, NULL);
    IF p_link_public THEN
        UPDATE briefcase.entries SET link_public = true, link_expires_at = p_link_expires_at
         WHERE org_id = p_drive AND entry_id = created;
    END IF;
    INSERT INTO link_folders VALUES (p_key, p_drive, created);
    RETURN created;
END;
$$;

-- The app's folder `apps/{app}/` in a drive, created on demand as the API
-- creates it (system folders owned by the drive's owner).
CREATE OR REPLACE FUNCTION pg_temp.link_app_folder(p_drive text, p_app text)
RETURNS uuid
LANGUAGE plpgsql
AS $$
DECLARE
    root uuid := pg_temp.link_drive(p_drive);
    owner_kind text := pg_temp.link_kind_of(p_drive);
    container uuid;
    folder uuid;
BEGIN
    SELECT entry_id INTO container FROM briefcase.entries
     WHERE org_id = p_drive AND parent_id = root AND system_kind = 'apps_root'
       AND name = 'apps' AND deleted_at IS NULL;
    IF container IS NULL THEN
        IF EXISTS (SELECT 1 FROM briefcase.entries
                    WHERE org_id = p_drive AND parent_id = root AND name = 'apps'
                      AND deleted_at IS NULL) THEN
            RAISE EXCEPTION 'drive % has a folder named apps at its top that is not the apps folder', p_drive;
        END IF;
        container := pg_temp.link_new_folder(p_drive, root, 'apps', 'apps_root', owner_kind, p_drive, NULL);
    END IF;
    SELECT entry_id INTO folder FROM briefcase.entries
     WHERE org_id = p_drive AND parent_id = container AND system_kind = 'app_root'
       AND name = p_app AND origin_app_id = p_app AND deleted_at IS NULL;
    IF folder IS NULL THEN
        folder := pg_temp.link_new_folder(p_drive, container, p_app, 'app_root', owner_kind, p_drive, p_app);
    END IF;
    RETURN folder;
END;
$$;
