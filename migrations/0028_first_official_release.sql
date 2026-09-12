-- v1: immutable versions, year-long logs, application boundaries and link sharing.
DROP TRIGGER audit_events_retain_latest_entry_events ON briefcase.audit_events;
DROP FUNCTION briefcase.retain_latest_entry_audits();
CREATE INDEX audit_events_retention_idx ON briefcase.audit_events (occurred_at);
ALTER TABLE briefcase.entries DROP CONSTRAINT entries_system_kind_check;
ALTER TABLE briefcase.entries ADD CONSTRAINT entries_system_kind_check CHECK (
 system_kind IS NULL OR system_kind IN ('public_root','private_root','tag_root','actor_root',
 'app_container','apps_root','app_root','app_public','app_private','app_actor'));
CREATE UNIQUE INDEX entries_apps_root_uidx ON briefcase.entries(org_id) WHERE system_kind = 'apps_root';
ALTER TABLE briefcase.entries ADD COLUMN link_public boolean NOT NULL DEFAULT false;
ALTER TABLE briefcase.entries ADD CONSTRAINT entries_link_public_safe CHECK (NOT link_public OR system_kind IS NULL);
CREATE INDEX entries_public_links_idx ON briefcase.entries(org_id,entry_id) WHERE link_public AND deleted_at IS NULL;
ALTER TABLE briefcase.entry_versions ADD COLUMN content_sha256 text CHECK (content_sha256 ~ '^[0-9a-f]{64}$');
CREATE OR REPLACE FUNCTION briefcase.validate_entry_parent()
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

    IF (NEW.root_type <> parent_row.root_type
        OR NEW.tag_id IS DISTINCT FROM parent_row.tag_id)
        AND NOT (parent_row.system_kind = 'app_root' AND NEW.system_kind IN ('app_public', 'app_private'))
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


-- Record a child's change in its parent's log in the same transaction. Metadata
-- snapshots keep the event meaningful after a move or permanent deletion.
CREATE FUNCTION briefcase.log_parent_change() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, briefcase AS $$
DECLARE parent uuid;
BEGIN
 IF NEW.entry_id IS NULL OR NEW.action LIKE 'child.%' OR NEW.action LIKE '%accessed%' OR NEW.action LIKE '%downloaded%' THEN RETURN NULL; END IF;
 SELECT parent_id INTO parent FROM briefcase.entries WHERE org_id=NEW.org_id AND entry_id=NEW.entry_id;
 IF parent IS NOT NULL THEN
  INSERT INTO briefcase.audit_events(org_id,audit_id,entry_id,actor_type,actor_id,origin_app_id,action,request_id,metadata,occurred_at)
  VALUES(NEW.org_id,gen_random_uuid(),parent,NEW.actor_type,NEW.actor_id,NEW.origin_app_id,'child.'||NEW.action,NEW.request_id,
    NEW.metadata || jsonb_build_object('entry_id',NEW.entry_id),NEW.occurred_at);
 END IF;
 RETURN NULL;
END; $$;
CREATE TRIGGER audit_events_parent_changes AFTER INSERT ON briefcase.audit_events
FOR EACH ROW EXECUTE FUNCTION briefcase.log_parent_change();
