ALTER TABLE briefcase.testing_environments
 ADD COLUMN iam_control_version bigint,
 ADD COLUMN iam_cleaned_at timestamptz,
 ADD COLUMN iam_sync_pending boolean NOT NULL DEFAULT false;
-- Creator provenance comes from verified IAM metadata; discovering a test world
-- must not create a production membership projection just to satisfy this FK.
DO $$ DECLARE c record; BEGIN
 FOR c IN SELECT conname FROM pg_constraint WHERE conrelid='briefcase.testing_environments'::regclass
 AND contype='f' AND confrelid='briefcase.organization_members'::regclass LOOP
 EXECUTE format('ALTER TABLE briefcase.testing_environments DROP CONSTRAINT %I',c.conname);
 END LOOP;
END $$;
-- IAM limits Unicode characters; legacy Briefcase inputs use byte limits.
DO $$ DECLARE c record; BEGIN
 FOR c IN SELECT conname FROM pg_constraint WHERE conrelid='briefcase.testing_environments'::regclass
 AND contype='c' AND (pg_get_constraintdef(oid) LIKE '%octet_length(name)%'
 OR pg_get_constraintdef(oid) LIKE '%octet_length(description)%') LOOP
 EXECUTE format('ALTER TABLE briefcase.testing_environments DROP CONSTRAINT %I',c.conname);
 END LOOP;
END $$;
ALTER TABLE briefcase.testing_environments
 ADD CHECK(name=btrim(name) AND CASE WHEN iam_control_version IS NULL
 THEN octet_length(name) BETWEEN 1 AND 100 ELSE char_length(name) BETWEEN 1 AND 64 END),
 ADD CHECK(description IS NULL OR CASE WHEN iam_control_version IS NULL
 THEN octet_length(description)<=1000 ELSE char_length(description)<=500 END);
DROP INDEX briefcase.testing_environments_active_name_uidx;
CREATE UNIQUE INDEX testing_environments_active_name_uidx
 ON briefcase.testing_environments(org_id,name COLLATE "C")
 WHERE status='active' AND iam_control_version IS NULL;
CREATE OR REPLACE FUNCTION briefcase.testing_environment_version_matches(selected uuid,expected_version bigint)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT EXISTS(SELECT 1 FROM briefcase.testing_environments WHERE environment_id=selected
 AND status='active' AND version=expected_version AND NOT iam_sync_pending);
$$;

CREATE FUNCTION briefcase.discovered_testing_webhook_candidates()
RETURNS TABLE(environment_id uuid,secret_ciphertext bytea,secret_nonce bytea)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT environment_id,iam_app_secret_ciphertext,iam_app_secret_nonce
 FROM briefcase.testing_environments WHERE status='active' AND iam_control_version IS NOT NULL;
$$;
REVOKE ALL ON FUNCTION briefcase.discovered_testing_webhook_candidates() FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.discovered_testing_webhook_candidates() TO briefcase_api;
END IF; END $$;

-- IAM reset invalidates every identity projection in the world. Retain tenant
-- bookkeeping and provider cleanup descriptors until object deletion completes.
CREATE FUNCTION briefcase.reset_current_iam_testing_environment()
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER
SET search_path=pg_catalog,briefcase AS $$
DECLARE
 selected_environment uuid := NULLIF(current_setting('briefcase.testing_environment_id',true),'')::uuid;
 original_org text := NULLIF(current_setting('briefcase.org_id',true),'');
 selected_org text;
 erased bigint := 0;
BEGIN
 IF selected_environment IS NULL OR original_org IS NULL
 OR original_org NOT LIKE selected_environment::text || ':%' THEN
 RAISE EXCEPTION USING ERRCODE='42501',MESSAGE='a testing-environment transaction context is required';
 END IF;
 FOR selected_org IN SELECT org_id FROM briefcase.organizations
 WHERE testing_environment_id=selected_environment LOOP
  PERFORM set_config('briefcase.org_id',selected_org,true);
  PERFORM briefcase.prepare_current_testing_environment_clean();
  erased := erased + briefcase.erase_current_testing_environment();
  DELETE FROM briefcase.organization_member_tags WHERE org_id=selected_org;
  DELETE FROM briefcase.organization_tags WHERE org_id=selected_org;
  DELETE FROM briefcase.organization_members WHERE org_id=selected_org;
 END LOOP;
 PERFORM set_config('briefcase.org_id',original_org,true);
 RETURN erased;
END $$;
REVOKE ALL ON FUNCTION briefcase.reset_current_iam_testing_environment() FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.reset_current_iam_testing_environment() TO briefcase_api;
END IF; END $$;
