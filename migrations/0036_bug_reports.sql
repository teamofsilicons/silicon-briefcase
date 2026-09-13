-- Reports are explicit user submissions, not telemetry. Test reports stay in
-- the test database and disappear with the organization during sandbox cleanup.
CREATE TABLE briefcase.bug_reports (
    org_id text NOT NULL REFERENCES briefcase.organizations(org_id) ON DELETE CASCADE,
    report_id uuid NOT NULL,
    actor_type text NOT NULL CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text NOT NULL,
    message text NOT NULL CHECK (octet_length(message) BETWEEN 1 AND 16384),
    pr_url text,
    isi text CHECK (isi IS NULL OR octet_length(isi) BETWEEN 1 AND 256),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, report_id)
);
ALTER TABLE briefcase.bug_reports ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.bug_reports FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.bug_reports
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT SELECT, INSERT, DELETE ON briefcase.bug_reports TO briefcase_api;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_worker') THEN
        GRANT SELECT, DELETE ON briefcase.bug_reports TO briefcase_worker;
    END IF;
END $$;

-- Cleaning and IAM reset preserve the organization, so explicitly erase its reports.
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
    PERFORM briefcase.prepare_current_testing_environment_delegated_cleanup();
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

    INSERT INTO briefcase.object_cleanup_jobs (
        org_id, cleanup_id, cleanup_kind, source_entry_id, source_version_id,
        source_upload_id, deletion_batch_id, storage_backend, storage_config_id,
        bucket_name, storage_region, storage_prefix, storage_role_arn,
        storage_encryption_mode, storage_kms_key_arn, object_key,
        object_version_id, provider_upload_id
    )
    SELECT version.org_id,
           md5('testing-clean:version:' || version.entry_id::text || ':' || version.version_id::text)::uuid,
           'version_delete', version.entry_id, version.version_id, NULL,
           entry.deletion_batch_id, version.storage_backend, version.storage_config_id,
           version.bucket_name, version.storage_region, version.storage_prefix,
           configuration.role_arn, version.storage_encryption_mode,
           version.storage_kms_key_arn, version.object_key,
           version.object_version_id, NULL
      FROM briefcase.entry_versions AS version
      JOIN briefcase.entries AS entry
        ON entry.org_id = version.org_id
       AND entry.entry_id = version.entry_id
      LEFT JOIN briefcase.organization_storage_configs AS configuration
        ON configuration.org_id = version.org_id
       AND configuration.storage_config_id = version.storage_config_id
     WHERE version.org_id = selected_org
    ON CONFLICT DO NOTHING;

    INSERT INTO briefcase.object_cleanup_jobs (
        org_id, cleanup_id, cleanup_kind, source_entry_id, source_version_id,
        source_upload_id, deletion_batch_id, storage_backend, storage_config_id,
        bucket_name, storage_region, storage_prefix, storage_role_arn,
        storage_encryption_mode, storage_kms_key_arn, object_key,
        object_version_id, provider_upload_id
    )
    SELECT upload.org_id,
           md5('testing-clean:multipart:' || upload.upload_id::text)::uuid,
           'multipart_abort', NULL, NULL, upload.upload_id, NULL,
           upload.storage_backend, upload.storage_config_id, upload.bucket_name,
           upload.storage_region, upload.storage_prefix, configuration.role_arn,
           upload.storage_encryption_mode, upload.storage_kms_key_arn,
           upload.object_key, NULL, upload.provider_upload_id
      FROM briefcase.multipart_uploads AS upload
      LEFT JOIN briefcase.organization_storage_configs AS configuration
        ON configuration.org_id = upload.org_id
       AND configuration.storage_config_id = upload.storage_config_id
     WHERE upload.org_id = selected_org
       AND upload.status <> 'completed'
    ON CONFLICT DO NOTHING;

    DELETE FROM briefcase.bug_reports WHERE org_id = selected_org;
    GET DIAGNOSTICS affected_rows = ROW_COUNT;
    erased_rows := erased_rows + affected_rows;

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

    -- Jobs whose provider outcome was already confirmed no longer need source
    -- metadata. Pending/processing jobs remain durable across this reset.
    DELETE FROM briefcase.object_cleanup_jobs
     WHERE org_id = selected_org AND status = 'object_deleted';
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
        org_id, daily_window, daily_upload_bytes, stored_bytes,
        daily_upload_limit_bytes, storage_limit_bytes
    ) VALUES (
        selected_org, (clock_timestamp() AT TIME ZONE 'UTC')::date,
        0, 0, NULL, 2147483648
    ) ON CONFLICT (org_id) DO UPDATE
        SET daily_window = EXCLUDED.daily_window,
            daily_upload_bytes = 0,
            stored_bytes = 0,
            daily_upload_limit_bytes = NULL,
            storage_limit_bytes = EXCLUDED.storage_limit_bytes;

    RETURN erased_rows;
END;
$$;
