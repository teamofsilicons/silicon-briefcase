-- Public spelling changes do not change entry UUIDs, identity UUIDs or content.
ALTER TABLE briefcase.testing_environments
 DROP CONSTRAINT testing_environments_iam_app_id_check;
ALTER TABLE briefcase.testing_environments
 ADD CONSTRAINT testing_environments_iam_app_id_check
 CHECK (iam_app_id ~ '^[a-z][a-z0-9_-]{0,79}$') NOT VALID;

CREATE TABLE briefcase.identifier_path_aliases (
 org_id text NOT NULL,
 legacy_path text NOT NULL,
 entry_id uuid NOT NULL,
 PRIMARY KEY(org_id,legacy_path),
 FOREIGN KEY(org_id,entry_id) REFERENCES briefcase.entries(org_id,entry_id) ON DELETE CASCADE,
 CHECK (length(legacy_path) BETWEEN 1 AND 2048)
);
ALTER TABLE briefcase.identifier_path_aliases ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.identifier_path_aliases FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.identifier_path_aliases
 USING(org_id=briefcase.current_org_id()) WITH CHECK(org_id=briefcase.current_org_id());

-- Resolve only to the current path; normal entry authorization/link-public checks
-- still run on the target. An alias never grants access or bypasses tenant RLS.
CREATE FUNCTION briefcase.resolve_identifier_path(requested text)
RETURNS text LANGUAGE sql STABLE SET search_path=pg_catalog,briefcase AS $$
 SELECT COALESCE((SELECT e.path FROM briefcase.identifier_path_aliases a
 JOIN briefcase.entries e USING(org_id,entry_id)
 WHERE a.org_id=briefcase.current_org_id() AND a.legacy_path=requested),requested);
$$;
CREATE FUNCTION briefcase.protect_identifier_path_alias()
RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,briefcase AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM briefcase.identifier_path_aliases
 WHERE org_id=NEW.org_id AND legacy_path=NEW.path AND entry_id<>NEW.entry_id) THEN
 RAISE EXCEPTION 'path is reserved by a migrated permanent link' USING ERRCODE='23505'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER entries_protect_identifier_alias
BEFORE INSERT OR UPDATE OF name,parent_id,path ON briefcase.entries
FOR EACH ROW EXECUTE FUNCTION briefcase.protect_identifier_path_alias();
REVOKE ALL ON FUNCTION briefcase.resolve_identifier_path(text) FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT SELECT ON briefcase.identifier_path_aliases TO briefcase_api;
 GRANT EXECUTE ON FUNCTION briefcase.resolve_identifier_path(text) TO briefcase_api;
END IF; END $$;

-- A deployment cannot silently allocate fresh keys for renamed retained actors.
CREATE OR REPLACE FUNCTION briefcase.resolve_iam_identity_key(p_environment uuid,p_kind text,p_public_id text,p_candidate uuid)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
DECLARE result uuid;
BEGIN
 IF p_environment IS DISTINCT FROM COALESCE(NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid,'00000000-0000-0000-0000-000000000000')
 OR p_kind NOT IN ('carbon','silicon','membership') OR p_public_id IS NULL OR length(p_public_id) NOT BETWEEN 1 AND 255
 OR p_candidate IS NULL OR p_candidate='00000000-0000-0000-0000-000000000000' THEN
 RAISE EXCEPTION 'invalid canonical identity context' USING ERRCODE='42501'; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.iam_identity_bindings WHERE environment_id=p_environment
 AND ((identity_kind='carbon' AND public_id NOT LIKE 'c:%')
 OR (identity_kind='silicon' AND public_id NOT LIKE 'si:%')
 OR (identity_kind='membership' AND public_id NOT SIMILAR TO '(c|si):%'))) THEN
 RAISE EXCEPTION 'public identifier migration incomplete' USING ERRCODE='55000'; END IF;
 SELECT local_id INTO result FROM briefcase.iam_identity_bindings
 WHERE environment_id=p_environment AND identity_kind=p_kind AND public_id=p_public_id;
 IF FOUND THEN RETURN result; END IF;
 IF EXISTS(SELECT 1 FROM briefcase.iam_identity_backfill WHERE environment_id=p_environment AND NOT verified) THEN
 RAISE EXCEPTION 'canonical identity backfill incomplete' USING ERRCODE='55000'; END IF;
 INSERT INTO briefcase.iam_identity_bindings VALUES(p_environment,p_kind,p_public_id,p_candidate)
 ON CONFLICT(environment_id,identity_kind,public_id) DO NOTHING;
 SELECT local_id INTO STRICT result FROM briefcase.iam_identity_bindings
 WHERE environment_id=p_environment AND identity_kind=p_kind AND public_id=p_public_id;
 RETURN result;
END $$;
