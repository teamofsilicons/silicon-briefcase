-- Anonymous links expose routing facts only; entry-level link policy still
-- decides visibility in the selected sandbox. Never return credentials here.
CREATE FUNCTION briefcase.public_testing_environment_version(candidate uuid, organization text)
RETURNS bigint
LANGUAGE sql STABLE SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT version FROM briefcase.testing_environments
     WHERE environment_id = candidate AND org_id = organization AND status = 'active' AND NOT iam_sync_pending
$$;
REVOKE ALL ON FUNCTION briefcase.public_testing_environment_version(uuid, text) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT EXECUTE ON FUNCTION briefcase.public_testing_environment_version(uuid, text) TO briefcase_api;
    END IF;
END $$;
