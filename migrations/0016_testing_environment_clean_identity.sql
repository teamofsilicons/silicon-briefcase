-- A clean environment must remain immediately usable by identities already
-- authenticated in its paired IAM plane. IAM webhooks are a change stream,
-- not a snapshot API, so deleting the organization/member/tag projection
-- would strand the environment until another IAM event happened to arrive.
--
-- Replace the v15 eraser with one that removes all Briefcase-owned state while
-- retaining only the current IAM directory projection. Deterministic system
-- entries are removed as well and are reconciled on the caller's next request.
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
    DELETE FROM briefcase.access_requests WHERE org_id = selected_org;
    DELETE FROM briefcase.permission_grants WHERE org_id = selected_org;
    DELETE FROM briefcase.multipart_parts WHERE org_id = selected_org;
    DELETE FROM briefcase.multipart_uploads WHERE org_id = selected_org;
    DELETE FROM briefcase.search_documents WHERE org_id = selected_org;
    DELETE FROM briefcase.entry_closure WHERE org_id = selected_org;
    DELETE FROM briefcase.idempotency_records WHERE org_id = selected_org;
    DELETE FROM briefcase.audit_events WHERE org_id = selected_org;
    DELETE FROM briefcase.outbox_events WHERE org_id = selected_org;
    DELETE FROM briefcase.webhook_receipts WHERE org_id = selected_org;
    DELETE FROM briefcase.object_cleanup_jobs WHERE org_id = selected_org;

    -- The usage trigger runs while its parent organization is still present.
    DELETE FROM briefcase.entry_versions WHERE org_id = selected_org;
    DELETE FROM briefcase.entries WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_storage_configs WHERE org_id = selected_org;

    -- Clear every consumption counter and restore the exact sandbox ceiling.
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

    -- The retained organization and directory rows are source-of-truth IAM
    -- projections, not Briefcase content. Returning one keeps the response's
    -- existing "selected data plane existed" meaning stable.
    RETURN 1;
END;
$$;
