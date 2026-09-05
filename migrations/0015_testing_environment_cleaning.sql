-- Cleaning a sandbox cannot rely on deleting the organization and allowing
-- cascades to choose their own order. Entry versions and multipart uploads
-- retain actor references, while deleting a version updates organization
-- usage; an organization-first cascade can therefore either violate a
-- foreign key or run the usage trigger after its parent is already gone.
--
-- Keep the destructive database operation behind a narrowly scoped function.
-- It only accepts an installed test-plane transaction context and removes all
-- child state in dependency order while the owning organization still exists.
CREATE FUNCTION briefcase.erase_current_testing_environment()
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
    erased_organizations bigint := 0;
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

    -- The current-version reference points back from entries to versions. It
    -- must remain deferred until both sides have been removed below.
    SET CONSTRAINTS briefcase.entries_current_version_fk DEFERRED;

    DELETE FROM briefcase.testing_environment_idempotency WHERE org_id = selected_org;
    DELETE FROM briefcase.testing_environments WHERE org_id = selected_org;
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

    -- Deleting versions while usage and the organization are present lets the
    -- stored-byte accounting trigger complete normally.
    DELETE FROM briefcase.entry_versions WHERE org_id = selected_org;
    DELETE FROM briefcase.entries WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_storage_configs WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_member_tags WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_tags WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_usage WHERE org_id = selected_org;
    DELETE FROM briefcase.organization_members WHERE org_id = selected_org;
    DELETE FROM briefcase.organizations
     WHERE org_id = selected_org
       AND testing_environment_id = selected_environment;
    GET DIAGNOSTICS erased_organizations = ROW_COUNT;

    RETURN erased_organizations;
END;
$$;

REVOKE ALL ON FUNCTION briefcase.erase_current_testing_environment() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.erase_current_testing_environment() TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.erase_current_testing_environment() TO briefcase_worker';
    END IF;
END;
$$;
