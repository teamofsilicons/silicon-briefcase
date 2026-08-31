-- Durable, cross-tenant object cleanup is intentionally separate from the
-- request path. A job snapshots the exact provider descriptor before any
-- external call so configuration rotation cannot redirect destructive work.
CREATE TABLE briefcase.object_cleanup_jobs (
    org_id text NOT NULL,
    cleanup_id uuid NOT NULL,
    cleanup_kind text NOT NULL CHECK (
        cleanup_kind IN ('multipart_abort', 'version_delete')
    ),
    source_entry_id uuid,
    source_version_id uuid,
    source_upload_id uuid,
    deletion_batch_id uuid,
    storage_backend text NOT NULL CHECK (
        storage_backend IN ('platform', 'organization')
    ),
    storage_config_id uuid,
    bucket_name text NOT NULL,
    storage_region text NOT NULL,
    storage_prefix text NOT NULL,
    storage_role_arn text,
    storage_encryption_mode text NOT NULL CHECK (
        storage_encryption_mode IN ('sse_s3', 'sse_kms')
    ),
    storage_kms_key_arn text,
    object_key text NOT NULL,
    object_version_id text,
    provider_upload_id text,
    status text NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'processing', 'object_deleted')
    ),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    available_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_token uuid,
    lease_expires_at timestamptz,
    last_error_code text,
    object_deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, cleanup_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    CHECK (
        (cleanup_kind = 'multipart_abort'
            AND source_entry_id IS NULL
            AND source_version_id IS NULL
            AND source_upload_id IS NOT NULL
            AND deletion_batch_id IS NULL
            AND object_version_id IS NULL
            AND provider_upload_id IS NOT NULL)
        OR
        (cleanup_kind = 'version_delete'
            AND source_entry_id IS NOT NULL
            AND source_version_id IS NOT NULL
            AND source_upload_id IS NULL
            AND provider_upload_id IS NULL)
    ),
    CHECK (
        (storage_backend = 'platform'
            AND storage_config_id IS NULL
            AND storage_role_arn IS NULL)
        OR
        (storage_backend = 'organization'
            AND storage_config_id IS NOT NULL
            AND storage_role_arn IS NOT NULL)
    ),
    CHECK (bucket_name = btrim(bucket_name)),
    CHECK (octet_length(bucket_name) BETWEEN 3 AND 255),
    CHECK (
        storage_region = btrim(storage_region)
        AND octet_length(storage_region) BETWEEN 1 AND 64
    ),
    CHECK (octet_length(storage_prefix) <= 1024),
    CHECK (storage_prefix !~ '(^|/)\.\.(/|$)'),
    CHECK (storage_prefix !~ '^/'),
    CHECK (
        storage_role_arn IS NULL
        OR octet_length(storage_role_arn) BETWEEN 20 AND 2048
    ),
    CHECK (
        (storage_encryption_mode = 'sse_kms')
            = (storage_kms_key_arn IS NOT NULL)
    ),
    CHECK (octet_length(object_key) BETWEEN 1 AND 2048),
    CHECK (
        object_version_id IS NULL
        OR octet_length(object_version_id) BETWEEN 1 AND 2048
    ),
    CHECK (
        provider_upload_id IS NULL
        OR octet_length(provider_upload_id) BETWEEN 1 AND 2048
    ),
    CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL)),
    CHECK (
        (status = 'processing' AND lease_token IS NOT NULL)
        OR (status <> 'processing' AND lease_token IS NULL)
    ),
    CHECK (
        (status = 'object_deleted') = (object_deleted_at IS NOT NULL)
    ),
    CHECK (
        last_error_code IS NULL
        OR (
            last_error_code = btrim(last_error_code)
            AND octet_length(last_error_code) BETWEEN 1 AND 128
        )
    )
);

CREATE UNIQUE INDEX object_cleanup_jobs_multipart_uidx
    ON briefcase.object_cleanup_jobs (org_id, source_upload_id)
    WHERE cleanup_kind = 'multipart_abort';

CREATE UNIQUE INDEX object_cleanup_jobs_version_uidx
    ON briefcase.object_cleanup_jobs (org_id, source_entry_id, source_version_id)
    WHERE cleanup_kind = 'version_delete';

CREATE INDEX object_cleanup_jobs_claim_idx
    ON briefcase.object_cleanup_jobs (available_at, created_at, org_id, cleanup_id)
    WHERE status = 'pending';

CREATE INDEX object_cleanup_jobs_expired_lease_idx
    ON briefcase.object_cleanup_jobs (lease_expires_at, org_id, cleanup_id)
    WHERE status = 'processing';

CREATE INDEX object_cleanup_jobs_deletion_batch_idx
    ON briefcase.object_cleanup_jobs (
        org_id,
        deletion_batch_id,
        status,
        source_entry_id,
        source_version_id
    )
    WHERE cleanup_kind = 'version_delete' AND deletion_batch_id IS NOT NULL;

CREATE TRIGGER object_cleanup_jobs_set_updated_at
BEFORE UPDATE ON briefcase.object_cleanup_jobs
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

ALTER TABLE briefcase.object_cleanup_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.object_cleanup_jobs FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.object_cleanup_jobs
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

-- `restored_from_version_id` is historical provenance, not ownership. Keeping
-- a self-FK would make the documented rolling 50-version retention impossible:
-- a retained restore could pin its source row forever after the source object
-- had been deleted. The opaque UUID remains immutable in the retained row.
ALTER TABLE briefcase.entry_versions
    DROP CONSTRAINT entry_versions_org_id_entry_id_restored_from_version_id_fkey;

-- The migration runs after the general runtime grants migration. The API needs
-- only INSERT for its transactional multipart-abort enqueue; the privileged
-- worker owns queue reads and state changes. Production deployments with
-- custom role names provision these minimum grants during migration rollout.
REVOKE ALL ON TABLE briefcase.object_cleanup_jobs FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT INSERT ON TABLE briefcase.object_cleanup_jobs TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE briefcase.object_cleanup_jobs TO briefcase_worker';
    END IF;
END;
$$;
