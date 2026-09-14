-- A sandbox's production control owner is not its data organization. This
-- credential-free helper reveals only active routing/version metadata. The
-- caller still installs the selected world + data org under RLS and enforces
-- public_entry/public_children link policy; a selector grants no entry access.
CREATE OR REPLACE FUNCTION briefcase.public_testing_environment_version(candidate uuid, organization text)
RETURNS bigint
LANGUAGE sql STABLE SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT version FROM briefcase.testing_environments
     WHERE environment_id = candidate AND status = 'active' AND NOT iam_sync_pending
       AND organization ~ '^[a-z0-9_-]{3,50}$'
$$;
REVOKE ALL ON FUNCTION briefcase.public_testing_environment_version(uuid, text) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT EXECUTE ON FUNCTION briefcase.public_testing_environment_version(uuid, text) TO briefcase_api;
    END IF;
END $$;
