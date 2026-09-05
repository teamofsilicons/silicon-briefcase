-- Clean retains the IAM organization/member projection so the next request can
-- immediately rebuild its roots. Final purge must remove that retained identity
-- as well, otherwise deleting the control row leaves an unreachable tenant in
-- the shared testing database.
CREATE FUNCTION briefcase.purge_current_testing_environment()
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    selected_org text := NULLIF(current_setting('briefcase.org_id', true), '');
    selected_environment uuid := NULLIF(
        current_setting('briefcase.testing_environment_id', true),
        ''
    )::uuid;
    affected_rows bigint := 0;
    erased_rows bigint := 0;
BEGIN
    -- This performs the same context validation and complete content erasure as
    -- an explicit clean, within this transaction.
    erased_rows := briefcase.erase_current_testing_environment();

    DELETE FROM briefcase.organization_member_tags WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.organization_tags WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.organization_usage WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.organization_members WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.organizations
     WHERE org_id = selected_org
       AND testing_environment_id = selected_environment;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;

    RETURN erased_rows;
END;
$$;

REVOKE ALL ON FUNCTION briefcase.purge_current_testing_environment() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.purge_current_testing_environment() TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.purge_current_testing_environment() TO briefcase_worker';
    END IF;
END;
$$;
