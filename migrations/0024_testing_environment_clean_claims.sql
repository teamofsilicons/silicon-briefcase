-- A test-environment clean owns every outstanding provider descriptor for its
-- logical reset. Invalidate claimed worker leases before the eraser snapshots
-- and removes source metadata so a stale preflight cancellation cannot delete
-- the only durable descriptor after the clean commits.
CREATE FUNCTION briefcase.prepare_current_testing_environment_clean()
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
    requeued_rows bigint := 0;
BEGIN
    IF selected_org IS NULL
        OR selected_environment IS NULL
        OR selected_org NOT LIKE selected_environment::text || ':%'
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'a testing-environment transaction context is required';
    END IF;

    IF NOT EXISTS (
        SELECT 1
          FROM briefcase.organizations AS organization
         WHERE organization.org_id = selected_org
           AND organization.testing_environment_id = selected_environment
    ) THEN
        RETURN 0;
    END IF;

    UPDATE briefcase.object_cleanup_jobs
       SET status = 'pending',
           available_at = GREATEST(
               COALESCE(lease_expires_at, clock_timestamp()),
               clock_timestamp()
           ),
           lease_token = NULL,
           lease_expires_at = NULL,
           last_error_code = NULL
     WHERE org_id = selected_org
       AND status = 'processing';
    GET DIAGNOSTICS requeued_rows = ROW_COUNT;
    RETURN requeued_rows;
END;
$$;

REVOKE ALL ON FUNCTION briefcase.prepare_current_testing_environment_clean() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.prepare_current_testing_environment_clean() TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.prepare_current_testing_environment_clean() TO briefcase_worker';
    END IF;
END;
$$;
