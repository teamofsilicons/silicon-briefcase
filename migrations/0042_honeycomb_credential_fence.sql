ALTER TABLE briefcase.testing_environments ADD COLUMN honeycomb_sync_pending boolean NOT NULL DEFAULT false;
CREATE OR REPLACE FUNCTION briefcase.testing_environment_version_matches(selected uuid,expected_version bigint)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT briefcase.honeycomb_environment_access(selected) AND EXISTS(
 SELECT 1 FROM briefcase.testing_environments WHERE environment_id=selected
 AND status='active' AND version=expected_version AND NOT iam_sync_pending AND NOT honeycomb_sync_pending);
$$;
CREATE OR REPLACE FUNCTION briefcase.public_testing_environment_version(candidate uuid, organization text)
RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT version FROM briefcase.testing_environments WHERE environment_id=candidate
 AND organization ~ '^[a-z0-9_-]{3,50}$'
 AND briefcase.testing_environment_version_matches(environment_id,version);
$$;
