-- Email addresses are learned only from IAM's verified self-contact projection.
CREATE TABLE briefcase.member_contacts (
 org_id text NOT NULL, actor_type text NOT NULL, actor_id text NOT NULL,
 email text NOT NULL CHECK (position('@' IN email)>1), verified_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(org_id,actor_type,actor_id),
 FOREIGN KEY(org_id,actor_type,actor_id) REFERENCES briefcase.organization_members ON DELETE CASCADE
);
CREATE INDEX member_contacts_email_idx ON briefcase.member_contacts(org_id,lower(email));
CREATE TABLE briefcase.tag_permission_grants (
 org_id text NOT NULL, entry_id uuid NOT NULL, grant_id uuid NOT NULL, tag_id text NOT NULL,
 access_mask smallint NOT NULL CHECK (access_mask IN (1,3,5,7)), inherits_to_descendants boolean NOT NULL,
 granted_by_type text NOT NULL, granted_by_id text NOT NULL,
 revoked_at timestamptz, revoked_by_type text, revoked_by_id text,
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 PRIMARY KEY(org_id,grant_id), FOREIGN KEY(org_id,entry_id) REFERENCES briefcase.entries ON DELETE CASCADE,
 FOREIGN KEY(org_id,tag_id) REFERENCES briefcase.organization_tags ON DELETE CASCADE
);
CREATE UNIQUE INDEX tag_permission_grants_active_idx ON briefcase.tag_permission_grants(org_id,entry_id,tag_id) WHERE revoked_at IS NULL;
ALTER TABLE briefcase.member_contacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.member_contacts FORCE ROW LEVEL SECURITY;
CREATE POLICY contacts_tenant ON briefcase.member_contacts USING(org_id=briefcase.current_org_id()) WITH CHECK(org_id=briefcase.current_org_id());
ALTER TABLE briefcase.tag_permission_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.tag_permission_grants FORCE ROW LEVEL SECURITY;
CREATE POLICY tag_grants_tenant ON briefcase.tag_permission_grants USING(org_id=briefcase.current_org_id()) WITH CHECK(org_id=briefcase.current_org_id());
CREATE VIEW briefcase.effective_permission_grants WITH(security_invoker=true) AS
 SELECT org_id,entry_id,grant_id,principal_type,principal_id,access_mask,inherits_to_descendants,granted_by_type,granted_by_id,revoked_at,revoked_by_type,revoked_by_id,created_at FROM briefcase.permission_grants
 UNION ALL
 SELECT g.org_id,g.entry_id,g.grant_id,m.actor_type,m.actor_id,g.access_mask,g.inherits_to_descendants,g.granted_by_type,g.granted_by_id,g.revoked_at,g.revoked_by_type,g.revoked_by_id,g.created_at
 FROM briefcase.tag_permission_grants g JOIN briefcase.organization_member_tags t ON t.org_id=g.org_id AND t.tag_id=g.tag_id
 JOIN briefcase.organization_tags tag ON tag.org_id=t.org_id AND tag.tag_id=t.tag_id AND tag.lifecycle_status='active'
 JOIN briefcase.organization_members m ON m.org_id=t.org_id AND m.actor_type=t.actor_type AND m.actor_id=t.actor_id AND m.membership_status='active';

CREATE FUNCTION briefcase.validate_invitation_rights() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,briefcase AS $$
BEGIN
 IF (NEW.access_mask & 8)<>0 THEN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='delete belongs to the creator and organization administrators'; END IF;
 IF (NEW.access_mask & 2)<>0 AND EXISTS(SELECT 1 FROM briefcase.entries WHERE org_id=NEW.org_id AND entry_id=NEW.entry_id AND entry_type='file') THEN
 RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='create access is valid only on folders'; END IF;
 RETURN NEW;
END; $$;
CREATE TRIGGER invitation_rights BEFORE INSERT OR UPDATE ON briefcase.permission_grants FOR EACH ROW EXECUTE FUNCTION briefcase.validate_invitation_rights();
CREATE TRIGGER tag_invitation_rights BEFORE INSERT OR UPDATE ON briefcase.tag_permission_grants FOR EACH ROW EXECUTE FUNCTION briefcase.validate_invitation_rights();

CREATE FUNCTION briefcase.enqueue_invitation_email() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,briefcase AS $$
BEGIN
 IF NEW.kind='access_granted' THEN
 INSERT INTO briefcase.outbox_events(org_id,event_id,topic,aggregate_type,aggregate_id,payload,available_at)
 VALUES(NEW.org_id,gen_random_uuid(),'briefcase.invitation-email.v1','notification',NEW.notification_id::text,
 jsonb_build_object('recipient_type',NEW.recipient_type,'recipient_id',NEW.recipient_id,'details',NEW.details),clock_timestamp());
 END IF;
 RETURN NULL;
END; $$;
CREATE TRIGGER notifications_email AFTER INSERT ON briefcase.notifications FOR EACH ROW EXECUTE FUNCTION briefcase.enqueue_invitation_email();
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_api') THEN
 GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.member_contacts,briefcase.tag_permission_grants TO briefcase_api;
 GRANT SELECT ON briefcase.effective_permission_grants TO briefcase_api;
 END IF;
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_worker') THEN
 GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.member_contacts,briefcase.tag_permission_grants TO briefcase_worker;
 GRANT SELECT ON briefcase.effective_permission_grants TO briefcase_worker;
 END IF;
END $$;
