-- One capability-scoped provider attempt, followed by a freshly IAM-authorized
-- publication. These rows are also the durable cleanup descriptors; unlike
-- file versions, unpublished objects must survive a logical test-plane clean.
CREATE TABLE briefcase.delegated_uploads (
    org_id text NOT NULL REFERENCES briefcase.organizations (org_id) ON DELETE CASCADE,
    upload_id uuid NOT NULL,
    operation_id uuid NOT NULL,
    testing_environment_id uuid,
    control_version bigint NOT NULL DEFAULT 0,
    iam_organization_id uuid NOT NULL,
    iam_principal_id uuid NOT NULL,
    iam_membership_id uuid NOT NULL,
    actor_type text NOT NULL CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text NOT NULL,
    origin_app_id text NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    capability_hash bytea CHECK (octet_length(capability_hash) = 32),
    parent_id uuid NOT NULL,
    parent_path text NOT NULL,
    name text NOT NULL,
    content_type text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes BETWEEN 0 AND 5497558138880),
    expected_sha256 bytea NOT NULL CHECK (octet_length(expected_sha256) = 32),
    destination_entry_id uuid,
    proposed_entry_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'reserved' CHECK (status IN (
        'reserved', 'receiving', 'staged', 'committed',
        'cancelled', 'expired', 'cleanup_pending'
    )),
    expires_at timestamptz NOT NULL,
    lease_token uuid,
    lease_expires_at timestamptz,
    provider_write_started boolean NOT NULL DEFAULT false,
    provider_completion_started boolean NOT NULL DEFAULT false,
    -- Snapshots the configured timeout at every provider-call start so a
    -- worker or later deployment need not guess the old writer's timeout.
    provider_deadline_at timestamptz,
    provider_upload_id text,
    storage_backend text NOT NULL CHECK (storage_backend IN ('platform', 'organization')),
    storage_config_id uuid,
    bucket_name text NOT NULL,
    storage_region text NOT NULL,
    storage_prefix text NOT NULL,
    storage_role_arn text,
    storage_external_id text,
    storage_encryption_mode text NOT NULL CHECK (storage_encryption_mode IN ('sse_s3', 'sse_kms')),
    storage_kms_key_arn text,
    object_key text NOT NULL,
    object_version_id text,
    object_etag text,
    object_checksum_type text CHECK (object_checksum_type IN ('full_object', 'composite')),
    object_checksum_value text,
    published_entry_id uuid,
    -- A cleanup worker must acquire a separate lease, recheck the live status,
    -- and retain this entire descriptor whenever a provider outcome is unknown.
    cleanup_after timestamptz,
    cleanup_lease_token uuid,
    cleanup_lease_expires_at timestamptz,
    cleanup_attempts integer NOT NULL DEFAULT 0 CHECK (cleanup_attempts >= 0),
    cleanup_last_error text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, upload_id),
    UNIQUE (org_id, control_version, iam_organization_id, iam_principal_id,
            iam_membership_id, actor_type, actor_id, origin_app_id, operation_id),
    CHECK (upload_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK (operation_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CHECK ((testing_environment_id IS NULL AND control_version = 0)
        OR (testing_environment_id IS NOT NULL AND control_version > 0
            AND org_id LIKE testing_environment_id::text || ':%')),
    -- The digest can survive body reception for a safe pre-provider retry;
    -- before_provider_write removes it before any external mutation begins.
    CHECK (capability_hash IS NULL OR status IN ('reserved', 'receiving')),
    CHECK (status <> 'reserved' OR capability_hash IS NOT NULL),
    CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL)),
    CHECK ((status = 'receiving') = (lease_token IS NOT NULL)),
    CHECK ((cleanup_lease_token IS NULL) = (cleanup_lease_expires_at IS NULL)),
    CHECK (cleanup_lease_token IS NULL OR status = 'cleanup_pending'),
    CHECK (status <> 'cleanup_pending' OR cleanup_after IS NOT NULL),
    CHECK ((status = 'committed') = (published_entry_id IS NOT NULL)),
    CHECK (status NOT IN ('staged', 'committed')
        OR (provider_write_started AND provider_completion_started AND object_checksum_type IS NOT NULL
            AND object_checksum_value IS NOT NULL)),
    CHECK (status <> 'reserved' OR NOT provider_write_started),
    CHECK (NOT provider_completion_started OR provider_write_started),
    CHECK (NOT provider_write_started OR provider_deadline_at IS NOT NULL),
    CHECK (status NOT IN ('cancelled', 'expired') OR NOT provider_write_started),
    CHECK ((storage_backend = 'platform' AND storage_config_id IS NULL
            AND storage_role_arn IS NULL AND storage_external_id IS NULL)
        OR (storage_backend = 'organization' AND storage_config_id IS NOT NULL
            AND storage_role_arn IS NOT NULL AND storage_external_id IS NOT NULL)),
    CHECK ((storage_encryption_mode = 'sse_kms') = (storage_kms_key_arn IS NOT NULL)),
    CHECK (octet_length(actor_id) BETWEEN 1 AND 1024),
    CHECK (octet_length(origin_app_id) BETWEEN 1 AND 1024),
    CHECK (octet_length(parent_path) <= 8192),
    CHECK (octet_length(name) BETWEEN 1 AND 255 AND position('/' IN name) = 0),
    CHECK (octet_length(content_type) BETWEEN 1 AND 255),
    CHECK (octet_length(bucket_name) BETWEEN 3 AND 255),
    CHECK (octet_length(storage_region) BETWEEN 1 AND 64),
    CHECK (octet_length(storage_prefix) <= 1024 AND storage_prefix !~ '(^|/)\.\.(/|$)' AND storage_prefix !~ '^/'),
    CHECK (octet_length(object_key) BETWEEN 1 AND 2048),
    CHECK (provider_upload_id IS NULL OR octet_length(provider_upload_id) BETWEEN 1 AND 2048),
    CHECK (object_version_id IS NULL OR octet_length(object_version_id) BETWEEN 1 AND 2048),
    CHECK (cleanup_last_error IS NULL OR octet_length(cleanup_last_error) BETWEEN 1 AND 128)
);

CREATE INDEX delegated_uploads_staging_budget_idx
    ON briefcase.delegated_uploads (org_id, status)
    INCLUDE (size_bytes)
    WHERE status IN ('reserved', 'receiving', 'staged', 'cleanup_pending');
CREATE INDEX delegated_uploads_expiration_idx
    ON briefcase.delegated_uploads (expires_at, org_id, upload_id)
    WHERE status IN ('reserved', 'receiving', 'staged');
CREATE INDEX delegated_uploads_writer_lease_idx
    ON briefcase.delegated_uploads (lease_expires_at, org_id, upload_id)
    WHERE status = 'receiving';
CREATE INDEX delegated_uploads_cleanup_idx
    ON briefcase.delegated_uploads (cleanup_after, org_id, upload_id)
    WHERE status = 'cleanup_pending';
CREATE INDEX delegated_uploads_testing_environment_idx
    ON briefcase.delegated_uploads (testing_environment_id, org_id, upload_id)
    WHERE testing_environment_id IS NOT NULL;

CREATE TRIGGER delegated_uploads_set_updated_at
BEFORE UPDATE ON briefcase.delegated_uploads
FOR EACH ROW EXECUTE FUNCTION briefcase.set_updated_at();

ALTER TABLE briefcase.delegated_uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.delegated_uploads FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.delegated_uploads
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());
REVOKE ALL ON TABLE briefcase.delegated_uploads FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.delegated_uploads TO briefcase_api';
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE briefcase.delegated_uploads TO briefcase_worker';
    END IF;
END;
$$;
