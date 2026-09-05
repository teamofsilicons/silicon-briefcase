-- A signed IAM test webhook may project more than the control owner's
-- organization into one sandbox. Lifecycle re-pairing must therefore check
-- the entire selected environment, without granting cross-tenant row access.
CREATE FUNCTION briefcase.current_testing_environment_has_iam_projection()
RETURNS boolean
LANGUAGE plpgsql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
-- This does not confer RLS bypass. If the function owner cannot inspect all
-- rows, fail closed instead of returning a falsely empty environment.
SET row_security = off
AS $$
DECLARE
    selected_org text := NULLIF(current_setting('briefcase.org_id', true), '');
    selected_environment uuid := NULLIF(
        current_setting('briefcase.testing_environment_id', true),
        ''
    )::uuid;
BEGIN
    IF selected_org IS NULL
        OR selected_environment IS NULL
        OR selected_org NOT LIKE selected_environment::text || ':%'
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'a testing-environment transaction context is required';
    END IF;

    RETURN EXISTS (
        SELECT 1
          FROM briefcase.organizations AS organization
         WHERE organization.testing_environment_id = selected_environment
    );
END;
$$;

REVOKE ALL ON FUNCTION briefcase.current_testing_environment_has_iam_projection() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.current_testing_environment_has_iam_projection() TO briefcase_api';
    END IF;
END;
$$;
