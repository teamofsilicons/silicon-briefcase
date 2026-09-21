-- Honeycomb owns the lifecycle; these rows outlive purged data as replay fences.
CREATE TABLE briefcase.honeycomb_environments (
 environment_id uuid PRIMARY KEY,
 org_id text NOT NULL,
 environment_revision bigint NOT NULL CHECK(environment_revision > 0),
 generation bigint NOT NULL CHECK(generation > 0),
 key_version bigint NOT NULL CHECK(key_version > 0),
 state text NOT NULL CHECK(state IN ('active','disabled','cleaning','purged')),
 operation_id uuid NOT NULL,
 testing_key_ciphertext bytea NOT NULL,
 testing_key_nonce bytea NOT NULL,
 require_iam_clean boolean NOT NULL DEFAULT false,
 iam_cleaned_before timestamptz,
 last_activity_at timestamptz,
 activity_reported_at timestamptz
);
CREATE TABLE briefcase.honeycomb_operations (
 environment_id uuid NOT NULL REFERENCES briefcase.honeycomb_environments(environment_id),
 operation_id uuid NOT NULL,
 org_id text NOT NULL,
 request_hash bytea NOT NULL CHECK(octet_length(request_hash)=32),
 receipt jsonb NOT NULL,
 cleanup_queued boolean NOT NULL DEFAULT false,
 PRIMARY KEY(environment_id,operation_id)
);
ALTER TABLE briefcase.honeycomb_environments ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.honeycomb_environments FORCE ROW LEVEL SECURITY;
ALTER TABLE briefcase.honeycomb_operations ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.honeycomb_operations FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.honeycomb_environments
 USING(org_id=briefcase.current_org_id()) WITH CHECK(org_id=briefcase.current_org_id());
CREATE POLICY tenant_isolation ON briefcase.honeycomb_operations
 USING(org_id=briefcase.current_org_id()) WITH CHECK(org_id=briefcase.current_org_id());
CREATE FUNCTION briefcase.honeycomb_environment_access(selected uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT NOT EXISTS(SELECT 1 FROM briefcase.honeycomb_environments
 WHERE environment_id=selected AND state<>'active');
$$;
CREATE OR REPLACE FUNCTION briefcase.testing_environment_version_matches(selected uuid,expected_version bigint)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT briefcase.honeycomb_environment_access(selected) AND EXISTS(
 SELECT 1 FROM briefcase.testing_environments WHERE environment_id=selected
 AND status='active' AND version=expected_version AND NOT iam_sync_pending);
$$;
-- Cleanup covers every organization projected inside the selected environment.
CREATE FUNCTION briefcase.honeycomb_cleanup_pending()
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
DECLARE selected uuid := nullif(current_setting('briefcase.testing_environment_id',true),'')::uuid;
BEGIN
 IF selected IS NULL THEN RAISE EXCEPTION USING ERRCODE='42501',MESSAGE='test context required'; END IF;
 RETURN EXISTS(SELECT 1 FROM briefcase.object_cleanup_jobs j JOIN briefcase.organizations o USING(org_id)
 WHERE o.testing_environment_id=selected)
 OR EXISTS(SELECT 1 FROM briefcase.delegated_uploads WHERE testing_environment_id=selected AND provider_write_started);
END $$;
CREATE FUNCTION briefcase.honeycomb_purge_data()
RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
DECLARE selected uuid := nullif(current_setting('briefcase.testing_environment_id',true),'')::uuid;
BEGIN
 IF selected IS NULL OR briefcase.honeycomb_cleanup_pending() THEN
 RAISE EXCEPTION USING ERRCODE='42501',MESSAGE='test cleanup must complete first'; END IF;
 DELETE FROM briefcase.organizations WHERE testing_environment_id=selected;
END $$;
REVOKE ALL ON FUNCTION briefcase.honeycomb_environment_access(uuid),
 briefcase.honeycomb_cleanup_pending(),briefcase.honeycomb_purge_data() FROM PUBLIC;
DO $$ BEGIN
 IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT SELECT,INSERT,UPDATE ON briefcase.honeycomb_environments,briefcase.honeycomb_operations TO briefcase_api;
 GRANT EXECUTE ON FUNCTION briefcase.honeycomb_environment_access(uuid),briefcase.honeycomb_cleanup_pending(),briefcase.honeycomb_purge_data() TO briefcase_api;
 END IF;
END $$;
