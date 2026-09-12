-- Folder logs retain changes beneath every depth, including later deletions.
CREATE OR REPLACE FUNCTION briefcase.log_parent_change() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,briefcase AS $$
BEGIN
 IF NEW.entry_id IS NULL OR NEW.action LIKE 'child.%' OR NEW.action LIKE '%accessed%' OR NEW.action LIKE '%downloaded%' THEN RETURN NULL; END IF;
 INSERT INTO briefcase.audit_events(org_id,audit_id,entry_id,actor_type,actor_id,origin_app_id,action,request_id,metadata,occurred_at)
 SELECT NEW.org_id,gen_random_uuid(),c.ancestor_id,NEW.actor_type,NEW.actor_id,NEW.origin_app_id,'child.'||NEW.action,NEW.request_id,
    NEW.metadata || jsonb_build_object('entry_id',NEW.entry_id),NEW.occurred_at
 FROM briefcase.entry_closure c WHERE c.org_id=NEW.org_id AND c.descendant_id=NEW.entry_id AND c.depth>0;
 RETURN NULL;
END; $$;
