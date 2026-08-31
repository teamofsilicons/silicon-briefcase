CREATE TABLE briefcase.organization_storage_configs (
    org_id text NOT NULL,
    storage_config_id uuid NOT NULL,
    status text NOT NULL CHECK (
        status IN ('validating', 'active', 'failed', 'superseded', 'disabled')
    ),
    bucket_name text NOT NULL,
    region text NOT NULL,
    role_arn text NOT NULL,
    bucket_prefix text NOT NULL,
    aws_account_id text NOT NULL,
    encryption_mode text NOT NULL CHECK (encryption_mode IN ('sse_s3', 'sse_kms')),
    kms_key_arn text,
    validated_at timestamptz,
    validation_failure_code text,
    validation_failure_reason text,
    created_by_type text NOT NULL CHECK (created_by_type IN ('carbon', 'silicon')),
    created_by_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, storage_config_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, created_by_type, created_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    CHECK (bucket_name = btrim(bucket_name)),
    CHECK (octet_length(bucket_name) BETWEEN 3 AND 63),
    CHECK (region = btrim(region) AND octet_length(region) BETWEEN 1 AND 64),
    CHECK (role_arn = btrim(role_arn) AND octet_length(role_arn) BETWEEN 20 AND 2048),
    CHECK (aws_account_id ~ '^[0-9]{12}$'),
    CHECK (bucket_prefix !~ '(^|/)\.\.(/|$)'),
    CHECK (bucket_prefix !~ '^/'),
    CHECK ((encryption_mode = 'sse_kms') = (kms_key_arn IS NOT NULL)),
    CHECK (
        (status = 'failed' AND validation_failure_code IS NOT NULL)
        OR (status <> 'failed' AND validation_failure_code IS NULL AND validation_failure_reason IS NULL)
    ),
    CHECK (validation_failure_reason IS NULL OR octet_length(validation_failure_reason) <= 2000)
);

CREATE UNIQUE INDEX organization_storage_configs_active_uidx
    ON briefcase.organization_storage_configs (org_id)
    WHERE status = 'active';

CREATE INDEX organization_storage_configs_status_idx
    ON briefcase.organization_storage_configs (org_id, status, updated_at DESC);

CREATE TRIGGER organization_storage_configs_set_updated_at
BEFORE UPDATE ON briefcase.organization_storage_configs
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE FUNCTION briefcase.protect_storage_config_descriptor()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF (OLD.bucket_name, OLD.region, OLD.role_arn, OLD.bucket_prefix,
        OLD.aws_account_id, OLD.encryption_mode, OLD.kms_key_arn)
       IS DISTINCT FROM
       (NEW.bucket_name, NEW.region, NEW.role_arn, NEW.bucket_prefix,
        NEW.aws_account_id, NEW.encryption_mode, NEW.kms_key_arn) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'storage configuration descriptors are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER organization_storage_configs_protect_descriptor
BEFORE UPDATE ON briefcase.organization_storage_configs
FOR EACH ROW
EXECUTE FUNCTION briefcase.protect_storage_config_descriptor();

CREATE TABLE briefcase.entry_versions (
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    version_id uuid NOT NULL,
    version_number bigint NOT NULL CHECK (version_number > 0),
    source text NOT NULL CHECK (source IN ('upload', 'restore')),
    restored_from_version_id uuid,
    storage_backend text NOT NULL CHECK (storage_backend IN ('platform', 'organization')),
    storage_config_id uuid,
    bucket_name text NOT NULL,
    storage_region text NOT NULL,
    storage_prefix text NOT NULL,
    storage_encryption_mode text NOT NULL CHECK (
        storage_encryption_mode IN ('sse_s3', 'sse_kms')
    ),
    storage_kms_key_arn text,
    object_key text NOT NULL,
    object_version_id text,
    etag text,
    checksum_algorithm text NOT NULL CHECK (checksum_algorithm IN ('sha256')),
    checksum_type text NOT NULL CHECK (checksum_type IN ('full_object', 'composite')),
    checksum_value text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    content_type text NOT NULL,
    created_by_type text NOT NULL CHECK (created_by_type IN ('carbon', 'silicon')),
    created_by_id text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, entry_id, version_id),
    UNIQUE (org_id, entry_id, version_number),
    FOREIGN KEY (org_id, entry_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, storage_config_id)
        REFERENCES briefcase.organization_storage_configs (org_id, storage_config_id),
    FOREIGN KEY (org_id, created_by_type, created_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, entry_id, restored_from_version_id)
        REFERENCES briefcase.entry_versions (org_id, entry_id, version_id)
        DEFERRABLE INITIALLY IMMEDIATE,
    CHECK ((storage_backend = 'organization') = (storage_config_id IS NOT NULL)),
    CHECK (octet_length(bucket_name) BETWEEN 3 AND 255),
    CHECK (octet_length(storage_region) BETWEEN 1 AND 64),
    CHECK (octet_length(storage_prefix) <= 1024),
    CHECK (storage_prefix !~ '(^|/)\.\.(/|$)'),
    CHECK (storage_prefix !~ '^/'),
    CHECK (
        (storage_encryption_mode = 'sse_kms') = (storage_kms_key_arn IS NOT NULL)
    ),
    CHECK (octet_length(object_key) BETWEEN 1 AND 2048),
    CHECK (octet_length(checksum_value) BETWEEN 1 AND 512),
    CHECK (octet_length(content_type) BETWEEN 1 AND 255),
    CHECK ((source = 'restore') = (restored_from_version_id IS NOT NULL))
);

CREATE INDEX entry_versions_recent_idx
    ON briefcase.entry_versions (org_id, entry_id, version_number DESC);

CREATE INDEX entry_versions_storage_idx
    ON briefcase.entry_versions (org_id, storage_backend, storage_config_id);

CREATE FUNCTION briefcase.prevent_entry_version_update()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = '23514',
        MESSAGE = 'entry versions are immutable';
END;
$$;

CREATE TRIGGER entry_versions_are_immutable
BEFORE UPDATE ON briefcase.entry_versions
FOR EACH ROW
EXECUTE FUNCTION briefcase.prevent_entry_version_update();

ALTER TABLE briefcase.entries
    ADD CONSTRAINT entries_current_version_fk
    FOREIGN KEY (org_id, entry_id, current_version_id)
    REFERENCES briefcase.entry_versions (org_id, entry_id, version_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION briefcase.require_current_file_version()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    current_entry record;
BEGIN
    SELECT entry.entry_type, entry.current_version_id
      INTO current_entry
      FROM briefcase.entries AS entry
     WHERE entry.org_id = NEW.org_id
       AND entry.entry_id = NEW.entry_id;

    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF current_entry.entry_type = 'file' AND current_entry.current_version_id IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'a persisted file must reference its current content version';
    END IF;

    IF current_entry.entry_type = 'folder' AND current_entry.current_version_id IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'a folder cannot reference a content version';
    END IF;

    RETURN NULL;
END;
$$;

CREATE CONSTRAINT TRIGGER entries_require_current_file_version
AFTER INSERT OR UPDATE ON briefcase.entries
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION briefcase.require_current_file_version();

CREATE TABLE briefcase.permission_grants (
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    grant_id uuid NOT NULL,
    principal_type text NOT NULL CHECK (principal_type IN ('carbon', 'silicon')),
    principal_id text NOT NULL,
    access_level text NOT NULL CHECK (access_level IN ('read', 'write')),
    inherits_to_descendants boolean NOT NULL DEFAULT true,
    granted_by_type text NOT NULL CHECK (granted_by_type IN ('carbon', 'silicon')),
    granted_by_id text NOT NULL,
    revoked_at timestamptz,
    revoked_by_type text CHECK (revoked_by_type IN ('carbon', 'silicon')),
    revoked_by_id text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, grant_id),
    UNIQUE (org_id, entry_id, grant_id),
    FOREIGN KEY (org_id, entry_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, principal_type, principal_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, granted_by_type, granted_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, revoked_by_type, revoked_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id)
        MATCH FULL,
    CHECK ((revoked_at IS NULL) = (revoked_by_type IS NULL AND revoked_by_id IS NULL))
);

CREATE UNIQUE INDEX permission_grants_active_principal_uidx
    ON briefcase.permission_grants (org_id, entry_id, principal_type, principal_id)
    WHERE revoked_at IS NULL;

CREATE INDEX permission_grants_principal_idx
    ON briefcase.permission_grants (
        org_id,
        principal_type,
        principal_id,
        entry_id,
        access_level
    )
    WHERE revoked_at IS NULL;

CREATE TABLE briefcase.access_requests (
    org_id text NOT NULL,
    access_request_id uuid NOT NULL,
    entry_id uuid NOT NULL,
    requested_by_type text NOT NULL CHECK (requested_by_type IN ('carbon', 'silicon')),
    requested_by_id text NOT NULL,
    requested_access text NOT NULL CHECK (requested_access IN ('read', 'write')),
    reason text,
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied')),
    granted_access text CHECK (granted_access IN ('read', 'write')),
    decided_by_type text CHECK (decided_by_type IN ('carbon', 'silicon')),
    decided_by_id text,
    decided_at timestamptz,
    permission_grant_id uuid,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, access_request_id),
    FOREIGN KEY (org_id, entry_id)
        REFERENCES briefcase.entries (org_id, entry_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, requested_by_type, requested_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, decided_by_type, decided_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id)
        MATCH FULL,
    FOREIGN KEY (org_id, entry_id, permission_grant_id)
        REFERENCES briefcase.permission_grants (org_id, entry_id, grant_id),
    CHECK (reason IS NULL OR octet_length(reason) <= 4000),
    CHECK (
        (status = 'pending'
            AND granted_access IS NULL
            AND decided_by_type IS NULL
            AND decided_by_id IS NULL
            AND decided_at IS NULL
            AND permission_grant_id IS NULL)
        OR (status = 'denied'
            AND granted_access IS NULL
            AND decided_by_type IS NOT NULL
            AND decided_by_id IS NOT NULL
            AND decided_at IS NOT NULL
            AND permission_grant_id IS NULL)
        OR (status = 'approved'
            AND granted_access IS NOT NULL
            AND decided_by_type IS NOT NULL
            AND decided_by_id IS NOT NULL
            AND decided_at IS NOT NULL
            AND permission_grant_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX access_requests_pending_actor_uidx
    ON briefcase.access_requests (
        org_id,
        entry_id,
        requested_by_type,
        requested_by_id
    )
    WHERE status = 'pending';

CREATE INDEX access_requests_decision_queue_idx
    ON briefcase.access_requests (org_id, status, created_at, access_request_id);

CREATE TRIGGER access_requests_set_updated_at
BEFORE UPDATE ON briefcase.access_requests
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE TABLE briefcase.multipart_uploads (
    org_id text NOT NULL,
    upload_id uuid NOT NULL,
    parent_entry_id uuid NOT NULL,
    owner_type text NOT NULL CHECK (owner_type IN ('carbon', 'silicon')),
    owner_id text NOT NULL,
    origin_app_id text,
    name text NOT NULL,
    content_type text NOT NULL,
    declared_size_bytes bigint NOT NULL
        CHECK (declared_size_bytes BETWEEN 104857601 AND 5497558138880),
    part_size_bytes bigint NOT NULL
        CHECK (part_size_bytes BETWEEN 8388608 AND 5368709120),
    expected_part_count integer NOT NULL
        CHECK (expected_part_count BETWEEN 1 AND 10000),
    storage_backend text NOT NULL CHECK (storage_backend IN ('platform', 'organization')),
    storage_config_id uuid,
    bucket_name text NOT NULL,
    storage_region text NOT NULL,
    storage_prefix text NOT NULL,
    storage_encryption_mode text NOT NULL CHECK (
        storage_encryption_mode IN ('sse_s3', 'sse_kms')
    ),
    storage_kms_key_arn text,
    object_key text NOT NULL,
    provider_upload_id text NOT NULL,
    status text NOT NULL DEFAULT 'initiated' CHECK (
        status IN ('initiated', 'uploading', 'completing', 'completed', 'aborted', 'expired')
    ),
    completed_entry_id uuid,
    expires_at timestamptz NOT NULL,
    completed_at timestamptz,
    aborted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, upload_id),
    FOREIGN KEY (org_id, parent_entry_id)
        REFERENCES briefcase.entries (org_id, entry_id),
    FOREIGN KEY (org_id, owner_type, owner_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    FOREIGN KEY (org_id, storage_config_id)
        REFERENCES briefcase.organization_storage_configs (org_id, storage_config_id),
    FOREIGN KEY (org_id, completed_entry_id)
        REFERENCES briefcase.entries (org_id, entry_id),
    CHECK (name = btrim(name)),
    CHECK (octet_length(name) BETWEEN 1 AND 255),
    CHECK (position('/' IN name) = 0),
    CHECK (name NOT IN ('.', '..')),
    CHECK (octet_length(content_type) BETWEEN 1 AND 255),
    CHECK ((storage_backend = 'organization') = (storage_config_id IS NOT NULL)),
    CHECK (octet_length(bucket_name) BETWEEN 3 AND 255),
    CHECK (octet_length(storage_region) BETWEEN 1 AND 64),
    CHECK (octet_length(storage_prefix) <= 1024),
    CHECK (storage_prefix !~ '(^|/)\.\.(/|$)'),
    CHECK (storage_prefix !~ '^/'),
    CHECK (
        (storage_encryption_mode = 'sse_kms') = (storage_kms_key_arn IS NOT NULL)
    ),
    CHECK (octet_length(object_key) BETWEEN 1 AND 2048),
    CHECK (octet_length(provider_upload_id) BETWEEN 1 AND 2048),
    CHECK (expires_at > created_at),
    CHECK (
        (status = 'completed' AND completed_entry_id IS NOT NULL AND completed_at IS NOT NULL)
        OR (status <> 'completed' AND completed_entry_id IS NULL AND completed_at IS NULL)
    ),
    CHECK ((status = 'aborted') = (aborted_at IS NOT NULL))
);

CREATE INDEX multipart_uploads_expiry_idx
    ON briefcase.multipart_uploads (expires_at, org_id, upload_id)
    WHERE status IN ('initiated', 'uploading', 'completing');

CREATE INDEX multipart_uploads_owner_idx
    ON briefcase.multipart_uploads (org_id, owner_type, owner_id, created_at DESC);

CREATE TRIGGER multipart_uploads_set_updated_at
BEFORE UPDATE ON briefcase.multipart_uploads
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

CREATE FUNCTION briefcase.protect_multipart_storage_descriptor()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF (OLD.storage_backend, OLD.storage_config_id, OLD.bucket_name,
        OLD.storage_region, OLD.storage_prefix, OLD.storage_encryption_mode,
        OLD.storage_kms_key_arn, OLD.object_key, OLD.provider_upload_id)
       IS DISTINCT FROM
       (NEW.storage_backend, NEW.storage_config_id, NEW.bucket_name,
        NEW.storage_region, NEW.storage_prefix, NEW.storage_encryption_mode,
        NEW.storage_kms_key_arn, NEW.object_key, NEW.provider_upload_id) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'multipart storage descriptors are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER multipart_uploads_protect_storage_descriptor
BEFORE UPDATE ON briefcase.multipart_uploads
FOR EACH ROW
EXECUTE FUNCTION briefcase.protect_multipart_storage_descriptor();

CREATE TABLE briefcase.multipart_parts (
    org_id text NOT NULL,
    upload_id uuid NOT NULL,
    part_number integer NOT NULL CHECK (part_number BETWEEN 1 AND 10000),
    etag text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes BETWEEN 1 AND 5368709120),
    checksum_sha256 bytea NOT NULL CHECK (octet_length(checksum_sha256) = 32),
    uploaded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, upload_id, part_number),
    FOREIGN KEY (org_id, upload_id)
        REFERENCES briefcase.multipart_uploads (org_id, upload_id)
        ON DELETE CASCADE,
    CHECK (octet_length(etag) BETWEEN 1 AND 1024)
);

CREATE INDEX multipart_parts_uploaded_idx
    ON briefcase.multipart_parts (org_id, upload_id, uploaded_at);
