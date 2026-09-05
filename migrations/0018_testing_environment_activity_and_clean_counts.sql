-- Matching a root key selects a plane but is not itself successful actor
-- authentication. Activity is recorded explicitly by the API only after IAM
-- auth succeeds (or after a root-authorized lifecycle route accepts the key).
CREATE OR REPLACE FUNCTION briefcase.testing_environment_by_root_digest(candidate bytea)
RETURNS TABLE (
    org_id text,
    environment_id uuid,
    name text,
    description text,
    key_generation bigint,
    control_version bigint,
    created_at timestamptz,
    iam_environment_id uuid,
    iam_app_id text,
    iam_environment_key_ciphertext bytea,
    iam_environment_key_nonce bytea,
    iam_app_secret_ciphertext bytea,
    iam_app_secret_nonce bytea
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT environment.org_id,
           environment.environment_id,
           environment.name,
           environment.description,
           environment.key_generation,
           environment.version,
           environment.created_at,
           environment.iam_environment_id,
           environment.iam_app_id,
           environment.iam_environment_key_ciphertext,
           environment.iam_environment_key_nonce,
           environment.iam_app_secret_ciphertext,
           environment.iam_app_secret_nonce
      FROM briefcase.testing_environments AS environment
     WHERE environment.root_key_digest = candidate
       AND environment.status = 'active'
$$;

-- Report the actual number of Briefcase-owned rows deleted. The retained IAM
-- directory projection and reset usage row are deliberately not counted.
CREATE OR REPLACE FUNCTION briefcase.erase_current_testing_environment()
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

    SET CONSTRAINTS briefcase.entries_current_version_fk DEFERRED;

    DELETE FROM briefcase.notifications WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.access_requests WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.permission_grants WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.multipart_parts WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.multipart_uploads WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.search_documents WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.entry_closure WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.idempotency_records WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.audit_events WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.outbox_events WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.webhook_receipts WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.object_cleanup_jobs WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;

    -- The usage trigger runs while its parent organization is still present.
    DELETE FROM briefcase.entry_versions WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.entries WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;
    DELETE FROM briefcase.organization_storage_configs WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;

    INSERT INTO briefcase.organization_usage AS usage_row (
        org_id,
        daily_window,
        daily_upload_bytes,
        stored_bytes,
        daily_upload_limit_bytes,
        storage_limit_bytes
    ) VALUES (
        selected_org,
        (clock_timestamp() AT TIME ZONE 'UTC')::date,
        0,
        0,
        NULL,
        2147483648
    ) ON CONFLICT (org_id) DO UPDATE
        SET daily_window = EXCLUDED.daily_window,
            daily_upload_bytes = 0,
            stored_bytes = 0,
            daily_upload_limit_bytes = NULL,
            storage_limit_bytes = EXCLUDED.storage_limit_bytes;

    RETURN erased_rows;
END;
$$;
