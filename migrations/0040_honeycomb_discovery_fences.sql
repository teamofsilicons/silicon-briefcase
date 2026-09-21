CREATE FUNCTION briefcase.honeycomb_owned_environment(selected uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT EXISTS(SELECT 1 FROM briefcase.honeycomb_environments WHERE environment_id=selected);
$$;
CREATE FUNCTION briefcase.honeycomb_active_environment_count(selected uuid)
RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT (SELECT count(*) FROM briefcase.honeycomb_environments WHERE state IN ('active','cleaning') AND environment_id<>selected)
 + (SELECT count(*) FROM briefcase.testing_environments e WHERE status='active' AND environment_id<>selected AND NOT briefcase.honeycomb_owned_environment(e.environment_id));
$$;
CREATE FUNCTION briefcase.honeycomb_discovery_matches(selected uuid,owner text,selected_key bigint,cleaned timestamptz)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT NOT briefcase.honeycomb_owned_environment(selected) OR EXISTS(
 SELECT 1 FROM briefcase.honeycomb_environments WHERE environment_id=selected AND org_id=owner AND state='active'
 AND key_version=selected_key AND (NOT require_iam_clean OR (cleaned IS NOT NULL AND (iam_cleaned_before IS NULL OR cleaned>iam_cleaned_before))));
$$;
REVOKE ALL ON FUNCTION briefcase.honeycomb_owned_environment(uuid),briefcase.honeycomb_active_environment_count(uuid),briefcase.honeycomb_discovery_matches(uuid,text,bigint,timestamptz) FROM PUBLIC;
DO $$ BEGIN
 IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.honeycomb_owned_environment(uuid),briefcase.honeycomb_active_environment_count(uuid),briefcase.honeycomb_discovery_matches(uuid,text,bigint,timestamptz) TO briefcase_api;
 END IF;
 IF to_regrole('briefcase_worker') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.honeycomb_owned_environment(uuid),briefcase.honeycomb_environment_access(uuid) TO briefcase_worker;
 END IF;
END $$;
