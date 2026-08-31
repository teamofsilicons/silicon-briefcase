CREATE SCHEMA IF NOT EXISTS briefcase;

CREATE FUNCTION briefcase.set_updated_at()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    NEW.updated_at = clock_timestamp();
    RETURN NEW;
END;
$$;

CREATE TABLE briefcase.organizations (
    org_id text PRIMARY KEY,
    iam_version bigint NOT NULL DEFAULT 0 CHECK (iam_version >= 0),
    lifecycle_status text NOT NULL DEFAULT 'active'
        CHECK (lifecycle_status IN ('active', 'suspended', 'removed')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (org_id = btrim(org_id)),
    CHECK (octet_length(org_id) BETWEEN 1 AND 255)
);

CREATE TRIGGER organizations_set_updated_at
BEFORE UPDATE ON briefcase.organizations
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.organization_members (
    org_id text NOT NULL,
    actor_type text NOT NULL CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text NOT NULL,
    org_role text NOT NULL DEFAULT 'member'
        CHECK (org_role IN ('owner', 'admin', 'member')),
    membership_status text NOT NULL DEFAULT 'active'
        CHECK (membership_status IN ('active', 'suspended', 'removed')),
    iam_version bigint NOT NULL DEFAULT 0 CHECK (iam_version >= 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    CHECK (actor_id = btrim(actor_id)),
    CHECK (octet_length(actor_id) BETWEEN 1 AND 255)
);

CREATE INDEX organization_members_active_idx
    ON briefcase.organization_members (org_id, actor_type, actor_id)
    WHERE membership_status = 'active';

CREATE TRIGGER organization_members_set_updated_at
BEFORE UPDATE ON briefcase.organization_members
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.organization_tags (
    org_id text NOT NULL,
    tag_id text NOT NULL,
    name text NOT NULL,
    lifecycle_status text NOT NULL DEFAULT 'active'
        CHECK (lifecycle_status IN ('active', 'removed')),
    iam_version bigint NOT NULL DEFAULT 0 CHECK (iam_version >= 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, tag_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    CHECK (tag_id = btrim(tag_id)),
    CHECK (octet_length(tag_id) BETWEEN 1 AND 255),
    CHECK (name = btrim(name)),
    CHECK (octet_length(name) BETWEEN 1 AND 255)
);

CREATE UNIQUE INDEX organization_tags_name_uidx
    ON briefcase.organization_tags (org_id, name COLLATE "C");

CREATE TRIGGER organization_tags_set_updated_at
BEFORE UPDATE ON briefcase.organization_tags
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.organization_member_tags (
    org_id text NOT NULL,
    actor_type text NOT NULL,
    actor_id text NOT NULL,
    tag_id text NOT NULL,
    iam_version bigint NOT NULL DEFAULT 0 CHECK (iam_version >= 0),
    assigned_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, actor_type, actor_id, tag_id),
    FOREIGN KEY (org_id, actor_type, actor_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, tag_id)
        REFERENCES briefcase.organization_tags (org_id, tag_id)
        ON DELETE CASCADE
);

CREATE INDEX organization_member_tags_tag_idx
    ON briefcase.organization_member_tags (org_id, tag_id, actor_type, actor_id);

CREATE TABLE briefcase.entries (
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    parent_id uuid,
    entry_type text NOT NULL CHECK (entry_type IN ('file', 'folder')),
    name text NOT NULL,
    root_type text NOT NULL CHECK (root_type IN ('public', 'private', 'tag')),
    tag_id text,
    system_kind text CHECK (
        system_kind IS NULL
        OR system_kind IN (
            'public_root',
            'private_root',
            'tag_root',
            'actor_root',
            'app_container'
        )
    ),
    owner_type text NOT NULL CHECK (owner_type IN ('carbon', 'silicon')),
    owner_id text NOT NULL,
    origin_app_id text,
    content_type text,
    size_bytes bigint CHECK (size_bytes IS NULL OR size_bytes >= 0),
    current_version_id uuid,
    created_by_type text NOT NULL CHECK (created_by_type IN ('carbon', 'silicon')),
    created_by_id text NOT NULL,
    updated_by_type text NOT NULL CHECK (updated_by_type IN ('carbon', 'silicon')),
    updated_by_id text NOT NULL,
    deletion_batch_id uuid,
    deleted_at timestamptz,
    purge_after timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, entry_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, parent_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        DEFERRABLE INITIALLY IMMEDIATE,
    FOREIGN KEY (org_id, tag_id)
        REFERENCES briefcase.organization_tags (org_id, tag_id),
    FOREIGN KEY (org_id, owner_type, owner_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, created_by_type, created_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, updated_by_type, updated_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    CHECK (name = btrim(name)),
    CHECK (octet_length(name) BETWEEN 1 AND 255),
    CHECK (position('/' IN name) = 0),
    CHECK (name NOT IN ('.', '..')),
    CHECK ((root_type = 'tag') = (tag_id IS NOT NULL)),
    CHECK (system_kind IS NULL OR entry_type = 'folder'),
    CHECK (system_kind <> 'public_root' OR (root_type = 'public' AND parent_id IS NULL)),
    CHECK (system_kind <> 'private_root' OR (root_type = 'private' AND parent_id IS NULL)),
    CHECK (system_kind <> 'tag_root' OR (root_type = 'tag' AND parent_id IS NULL)),
    CHECK (system_kind <> 'actor_root' OR root_type = 'private'),
    CHECK (system_kind <> 'app_container' OR root_type = 'private'),
    CHECK (
        (entry_type = 'folder' AND content_type IS NULL AND size_bytes IS NULL AND current_version_id IS NULL)
        OR (entry_type = 'file' AND size_bytes IS NOT NULL)
    ),
    CHECK (
        (deleted_at IS NULL AND purge_after IS NULL AND deletion_batch_id IS NULL)
        OR (
            deleted_at IS NOT NULL
            AND purge_after IS NOT NULL
            AND deletion_batch_id IS NOT NULL
            AND purge_after >= deleted_at
        )
    )
);

CREATE INDEX entries_parent_idx
    ON briefcase.entries (org_id, parent_id, entry_id);

CREATE INDEX entries_owner_idx
    ON briefcase.entries (org_id, owner_type, owner_id, entry_id);

CREATE INDEX entries_origin_app_idx
    ON briefcase.entries (org_id, origin_app_id, entry_id)
    WHERE origin_app_id IS NOT NULL;

CREATE INDEX entries_purge_idx
    ON briefcase.entries (purge_after, org_id, entry_id)
    WHERE purge_after IS NOT NULL;

CREATE UNIQUE INDEX entries_active_sibling_name_uidx
    ON briefcase.entries (org_id, parent_id, name COLLATE "C")
    NULLS NOT DISTINCT
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX entries_singleton_system_root_uidx
    ON briefcase.entries (org_id, system_kind)
    WHERE system_kind IN ('public_root', 'private_root');

CREATE UNIQUE INDEX entries_tag_root_uidx
    ON briefcase.entries (org_id, tag_id)
    WHERE system_kind = 'tag_root';

CREATE UNIQUE INDEX entries_actor_root_uidx
    ON briefcase.entries (org_id, owner_type, owner_id)
    WHERE system_kind = 'actor_root';

CREATE TABLE briefcase.entry_closure (
    org_id text NOT NULL,
    ancestor_id uuid NOT NULL,
    descendant_id uuid NOT NULL,
    depth integer NOT NULL CHECK (depth >= 0),
    PRIMARY KEY (org_id, ancestor_id, descendant_id),
    FOREIGN KEY (org_id, ancestor_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, descendant_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    CHECK ((ancestor_id = descendant_id) = (depth = 0))
);

CREATE INDEX entry_closure_descendant_idx
    ON briefcase.entry_closure (org_id, descendant_id, depth, ancestor_id);

CREATE FUNCTION briefcase.validate_entry_parent()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    parent_row briefcase.entries%ROWTYPE;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.org_id <> OLD.org_id OR NEW.entry_id <> OLD.entry_id THEN
            RAISE EXCEPTION USING
                ERRCODE = '23514',
                MESSAGE = 'entry organization and identifier are immutable';
        END IF;

        IF NEW.root_type <> OLD.root_type
            OR NEW.tag_id IS DISTINCT FROM OLD.tag_id
            OR NEW.system_kind IS DISTINCT FROM OLD.system_kind
        THEN
            RAISE EXCEPTION USING
                ERRCODE = '23514',
                MESSAGE = 'entry permission boundary and system kind are immutable';
        END IF;
    END IF;

    IF NEW.parent_id IS NULL THEN
        RETURN NEW;
    END IF;

    IF NEW.parent_id = NEW.entry_id THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'an entry cannot be its own parent';
    END IF;

    SELECT parent.*
      INTO parent_row
      FROM briefcase.entries AS parent
     WHERE parent.org_id = NEW.org_id
       AND parent.entry_id = NEW.parent_id
     FOR KEY SHARE;

    IF NOT FOUND THEN
        RAISE EXCEPTION USING
            ERRCODE = '23503',
            MESSAGE = 'entry parent does not exist in the organization';
    END IF;

    IF parent_row.entry_type <> 'folder' THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'entry parent must be a folder';
    END IF;

    IF NEW.deleted_at IS NULL AND parent_row.deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'an active entry cannot be placed below a deleted folder';
    END IF;

    IF NEW.root_type <> parent_row.root_type
        OR NEW.tag_id IS DISTINCT FROM parent_row.tag_id
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'an entry must inherit its parent permission boundary';
    END IF;

    IF TG_OP = 'UPDATE'
        AND NEW.parent_id IS DISTINCT FROM OLD.parent_id
        AND EXISTS (
            SELECT 1
              FROM briefcase.entry_closure AS closure
             WHERE closure.org_id = NEW.org_id
               AND closure.ancestor_id = NEW.entry_id
               AND closure.descendant_id = NEW.parent_id
        )
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'moving an entry below its descendant would create a cycle';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER entries_validate_parent
BEFORE INSERT OR UPDATE ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.validate_entry_parent();

CREATE FUNCTION briefcase.insert_entry_closure()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
    VALUES (NEW.org_id, NEW.entry_id, NEW.entry_id, 0);

    IF NEW.parent_id IS NOT NULL THEN
        INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
        SELECT NEW.org_id, parent_path.ancestor_id, NEW.entry_id, parent_path.depth + 1
          FROM briefcase.entry_closure AS parent_path
         WHERE parent_path.org_id = NEW.org_id
           AND parent_path.descendant_id = NEW.parent_id;
    END IF;

    RETURN NULL;
END;
$$;

CREATE TRIGGER entries_insert_closure
AFTER INSERT ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.insert_entry_closure();

CREATE FUNCTION briefcase.move_entry_closure()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    DELETE FROM briefcase.entry_closure AS link
    USING briefcase.entry_closure AS old_ancestor,
          briefcase.entry_closure AS subtree
    WHERE old_ancestor.org_id = NEW.org_id
      AND old_ancestor.descendant_id = NEW.entry_id
      AND old_ancestor.ancestor_id <> NEW.entry_id
      AND subtree.org_id = NEW.org_id
      AND subtree.ancestor_id = NEW.entry_id
      AND link.org_id = NEW.org_id
      AND link.ancestor_id = old_ancestor.ancestor_id
      AND link.descendant_id = subtree.descendant_id;

    IF NEW.parent_id IS NOT NULL THEN
        INSERT INTO briefcase.entry_closure (org_id, ancestor_id, descendant_id, depth)
        SELECT NEW.org_id,
               new_ancestor.ancestor_id,
               subtree.descendant_id,
               new_ancestor.depth + subtree.depth + 1
          FROM briefcase.entry_closure AS new_ancestor
          CROSS JOIN briefcase.entry_closure AS subtree
         WHERE new_ancestor.org_id = NEW.org_id
           AND new_ancestor.descendant_id = NEW.parent_id
           AND subtree.org_id = NEW.org_id
           AND subtree.ancestor_id = NEW.entry_id
        ON CONFLICT (org_id, ancestor_id, descendant_id)
        DO UPDATE SET depth = EXCLUDED.depth;
    END IF;

    RETURN NULL;
END;
$$;

CREATE TRIGGER entries_move_closure
AFTER UPDATE OF parent_id ON briefcase.entries
FOR EACH ROW
WHEN (NEW.parent_id IS DISTINCT FROM OLD.parent_id)
EXECUTE FUNCTION briefcase.move_entry_closure();

CREATE TRIGGER entries_set_updated_at
BEFORE UPDATE ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();
