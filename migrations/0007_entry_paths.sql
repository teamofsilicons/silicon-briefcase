-- Materialized organization-relative entry paths.
--
-- The product contract gives every entry a clean permanent URL that shows its
-- folder structure (`/org/{org_id}/private/{actor_id}/reports/q3.md`). Storing
-- the path makes that URL resolvable in one indexed lookup instead of a
-- recursive walk per request, and gives the filter language an indexable
-- `location:` predicate. Sibling-name uniqueness already guarantees that two
-- active entries can never share a path. The 2 KiB bound keeps the unique
-- index tuple inside PostgreSQL's btree row limit.

ALTER TABLE briefcase.entries ADD COLUMN path text;

-- The data statements below must observe every organization. `entries` forces
-- row-level security even for its owner, and a migration has no tenant
-- context, so the tenant policy is suspended for exactly this backfill.
ALTER TABLE briefcase.entries NO FORCE ROW LEVEL SECURITY;

WITH RECURSIVE tree AS (
    SELECT root.org_id, root.entry_id, root.name::text AS path
      FROM briefcase.entries AS root
     WHERE root.parent_id IS NULL
    UNION ALL
    SELECT child.org_id, child.entry_id, parent.path || '/' || child.name
      FROM tree AS parent
      JOIN briefcase.entries AS child
        ON child.org_id = parent.org_id
       AND child.parent_id = parent.entry_id
)
UPDATE briefcase.entries AS entry
   SET path = tree.path
  FROM tree
 WHERE tree.org_id = entry.org_id
   AND tree.entry_id = entry.entry_id;

ALTER TABLE briefcase.entries ALTER COLUMN path SET NOT NULL;

ALTER TABLE briefcase.entries
    ADD CONSTRAINT entries_path_shape CHECK (
        octet_length(path) BETWEEN 1 AND 2048
        AND path NOT LIKE '/%'
        AND path NOT LIKE '%/'
        AND path NOT LIKE '%//%'
    );

-- Equality resolves a permanent URL; the C collation also lets a `location:`
-- prefix filter (`path LIKE 'private/cos:tos/%'`) use the same index.
CREATE UNIQUE INDEX entries_active_path_uidx
    ON briefcase.entries (org_id, path COLLATE "C")
    WHERE deleted_at IS NULL;

-- Only the directly mutated row derives its path from its parent. A subtree
-- relocation rewrites descendant paths by prefix substitution, which must not
-- be overwritten here.
CREATE FUNCTION briefcase.derive_entry_path()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    parent_path text;
BEGIN
    IF TG_OP = 'UPDATE'
        AND NEW.name = OLD.name
        AND NEW.parent_id IS NOT DISTINCT FROM OLD.parent_id
    THEN
        RETURN NEW;
    END IF;

    IF NEW.parent_id IS NULL THEN
        NEW.path = NEW.name;
        RETURN NEW;
    END IF;

    -- The share lock serializes this derivation against a concurrent rename
    -- or move of the parent, which relocates the subtree by prefix rewrite.
    SELECT parent.path
      INTO parent_path
      FROM briefcase.entries AS parent
     WHERE parent.org_id = NEW.org_id
       AND parent.entry_id = NEW.parent_id
       FOR SHARE;

    IF parent_path IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23503',
            MESSAGE = 'entry parent does not exist in the organization';
    END IF;

    NEW.path = parent_path || '/' || NEW.name;
    RETURN NEW;
END;
$$;

CREATE TRIGGER entries_derive_path
BEFORE INSERT OR UPDATE ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.derive_entry_path();

CREATE FUNCTION briefcase.relocate_entry_subtree_paths()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF NEW.path = OLD.path THEN
        RETURN NULL;
    END IF;

    UPDATE briefcase.entries AS descendant
       SET path = NEW.path || substr(descendant.path, length(OLD.path) + 1)
      FROM briefcase.entry_closure AS subtree
     WHERE subtree.org_id = NEW.org_id
       AND subtree.ancestor_id = NEW.entry_id
       AND subtree.depth > 0
       AND descendant.org_id = subtree.org_id
       AND descendant.entry_id = subtree.descendant_id;

    RETURN NULL;
END;
$$;

CREATE TRIGGER entries_relocate_subtree_paths
AFTER UPDATE OF name, parent_id ON briefcase.entries
FOR EACH ROW
WHEN (NEW.path IS DISTINCT FROM OLD.path)
EXECUTE FUNCTION briefcase.relocate_entry_subtree_paths();

-- Relocating a descendant is not a metadata change of that descendant: an
-- ancestor rename must not restamp the whole subtree's `updated_at`.
CREATE FUNCTION briefcase.touch_entry_updated_at()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog
AS $$
BEGIN
    IF (to_jsonb(NEW) - 'path' - 'updated_at') = (to_jsonb(OLD) - 'path' - 'updated_at') THEN
        NEW.updated_at = OLD.updated_at;
    ELSE
        NEW.updated_at = clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$;

DROP TRIGGER entries_set_updated_at ON briefcase.entries;

CREATE TRIGGER entries_touch_updated_at
BEFORE UPDATE ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.touch_entry_updated_at();

-- The canonical containers appear in every permanent URL, so their names are
-- the lowercase path segments the contract shows. An organization that already
-- has an unrelated root folder with that exact name keeps its current name.
UPDATE briefcase.entries AS root
   SET name = canonical.name
  FROM (VALUES ('public_root', 'public'), ('private_root', 'private'))
       AS canonical (system_kind, name)
 WHERE root.system_kind = canonical.system_kind
   AND root.name <> canonical.name
   AND NOT EXISTS (
        SELECT 1
          FROM briefcase.entries AS sibling
         WHERE sibling.org_id = root.org_id
           AND sibling.parent_id IS NULL
           AND sibling.deleted_at IS NULL
           AND sibling.name = canonical.name
   );

ALTER TABLE briefcase.entries FORCE ROW LEVEL SECURITY;
