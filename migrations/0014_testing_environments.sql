-- Briefcase testing environments are controlled from the production database
-- while their ordinary filesystem rows live in a separately configured shared
-- test database. Secrets are encrypted by the API before they reach this table;
-- only keyed digests are used for lookups.
CREATE TABLE briefcase.testing_environments (
    org_id text NOT NULL,
    environment_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    created_by_type text NOT NULL CHECK (created_by_type IN ('carbon', 'silicon')),
    created_by_id text NOT NULL,
    iam_environment_id uuid NOT NULL,
    iam_app_id text NOT NULL,
    iam_environment_key_digest bytea NOT NULL CHECK (octet_length(iam_environment_key_digest) = 32),
    iam_environment_key_ciphertext bytea NOT NULL,
    iam_environment_key_nonce bytea NOT NULL CHECK (octet_length(iam_environment_key_nonce) = 12),
    iam_app_secret_ciphertext bytea NOT NULL,
    iam_app_secret_nonce bytea NOT NULL CHECK (octet_length(iam_app_secret_nonce) = 12),
    root_key_digest bytea,
    root_key_ciphertext bytea,
    root_key_nonce bytea,
    key_generation bigint NOT NULL DEFAULT 1 CHECK (key_generation > 0),
    key_rotated_at timestamptz,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'deleted')),
    last_activity_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    cleaned_at timestamptz,
    deleted_at timestamptz,
    purge_after timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, environment_id),
    UNIQUE (environment_id),
    UNIQUE (iam_environment_key_digest),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, created_by_type, created_by_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id),
    CHECK (name = btrim(name) AND octet_length(name) BETWEEN 1 AND 100),
    CHECK (description IS NULL OR octet_length(description) <= 1000),
    CHECK (iam_app_id = btrim(iam_app_id) AND octet_length(iam_app_id) BETWEEN 7 AND 131),
    CHECK (
        (status = 'active'
            AND root_key_digest IS NOT NULL
            AND octet_length(root_key_digest) = 32
            AND root_key_ciphertext IS NOT NULL
            AND root_key_nonce IS NOT NULL
            AND octet_length(root_key_nonce) = 12
            AND deleted_at IS NULL
            AND purge_after IS NULL)
        OR
        (status = 'deleted'
            AND root_key_digest IS NULL
            AND root_key_ciphertext IS NULL
            AND root_key_nonce IS NULL
            AND deleted_at IS NOT NULL
            AND purge_after IS NOT NULL
            AND purge_after >= deleted_at)
    )
);

CREATE UNIQUE INDEX testing_environments_active_name_uidx
    ON briefcase.testing_environments (org_id, name COLLATE "C")
    WHERE status = 'active';

CREATE UNIQUE INDEX testing_environments_active_root_key_uidx
    ON briefcase.testing_environments (root_key_digest)
    WHERE status = 'active';

CREATE INDEX testing_environments_lifecycle_idx
    ON briefcase.testing_environments (status, last_activity_at, purge_after, environment_id);

CREATE TRIGGER testing_environments_set_updated_at
BEFORE UPDATE ON briefcase.testing_environments
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

ALTER TABLE briefcase.testing_environments ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.testing_environments FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.testing_environments
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

-- Lifecycle responses can contain one-time root keys. Idempotency responses
-- therefore stay encrypted with the same service master key instead of using
-- the clear-text JSON columns on the general metadata idempotency table.
CREATE TABLE briefcase.testing_environment_idempotency (
    org_id text NOT NULL,
    authority_type text NOT NULL CHECK (authority_type IN ('carbon', 'silicon', 'root')),
    authority_id text NOT NULL,
    origin_app_id text NOT NULL DEFAULT '',
    operation text NOT NULL,
    idempotency_key text NOT NULL,
    request_hash bytea NOT NULL CHECK (octet_length(request_hash) = 32),
    environment_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'in_progress'
        CHECK (status IN ('in_progress', 'completed')),
    response_ciphertext bytea,
    response_nonce bytea,
    locked_until timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (
        org_id,
        authority_type,
        authority_id,
        origin_app_id,
        operation,
        idempotency_key
    ),
    FOREIGN KEY (org_id, environment_id)
        REFERENCES briefcase.testing_environments (org_id, environment_id)
        ON DELETE CASCADE
        DEFERRABLE INITIALLY DEFERRED,
    CHECK (authority_id = btrim(authority_id) AND octet_length(authority_id) BETWEEN 1 AND 255),
    CHECK (origin_app_id = btrim(origin_app_id) AND octet_length(origin_app_id) <= 255),
    CHECK (operation = btrim(operation) AND octet_length(operation) BETWEEN 1 AND 100),
    CHECK (octet_length(idempotency_key) BETWEEN 8 AND 255),
    CHECK (
        (status = 'in_progress'
            AND response_ciphertext IS NULL
            AND response_nonce IS NULL)
        OR
        (status = 'completed'
            AND response_ciphertext IS NOT NULL
            AND response_nonce IS NOT NULL
            AND octet_length(response_nonce) = 12)
    ),
    CHECK (expires_at > created_at)
);

CREATE INDEX testing_environment_idempotency_expiry_idx
    ON briefcase.testing_environment_idempotency (expires_at, org_id);

CREATE TRIGGER testing_environment_idempotency_set_updated_at
BEFORE UPDATE ON briefcase.testing_environment_idempotency
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

ALTER TABLE briefcase.testing_environment_idempotency ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.testing_environment_idempotency FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.testing_environment_idempotency
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

-- Data rows in the shared test database carry an explicit environment marker.
-- The existing org_id RLS key is additionally namespaced by environment UUID,
-- which prevents the same IAM organization handle in two sandboxes colliding.
ALTER TABLE briefcase.organizations
    ADD COLUMN testing_environment_id uuid;
ALTER TABLE briefcase.entries
    ADD COLUMN testing_environment_id uuid;

CREATE INDEX organizations_testing_environment_idx
    ON briefcase.organizations (testing_environment_id, org_id)
    WHERE testing_environment_id IS NOT NULL;
CREATE INDEX entries_testing_environment_idx
    ON briefcase.entries (testing_environment_id, org_id, entry_id)
    WHERE testing_environment_id IS NOT NULL;

CREATE FUNCTION briefcase.stamp_testing_environment()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    selected uuid := NULLIF(current_setting('briefcase.testing_environment_id', true), '')::uuid;
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.testing_environment_id := selected;
    ELSIF NEW.testing_environment_id IS DISTINCT FROM OLD.testing_environment_id THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'testing environment identity is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER organizations_stamp_testing_environment
BEFORE INSERT OR UPDATE ON briefcase.organizations
FOR EACH ROW
EXECUTE FUNCTION briefcase.stamp_testing_environment();

CREATE TRIGGER entries_stamp_testing_environment
BEFORE INSERT OR UPDATE ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.stamp_testing_environment();

-- These narrowly scoped SECURITY DEFINER functions are the only cross-tenant
-- API-runtime lookups. They reveal encrypted material only after a 256-bit
-- digest match and make activity tracking atomic with key acceptance.
CREATE FUNCTION briefcase.testing_environment_by_root_digest(candidate bytea)
RETURNS TABLE (
    org_id text,
    environment_id uuid,
    name text,
    description text,
    key_generation bigint,
    created_at timestamptz,
    iam_environment_id uuid,
    iam_app_id text,
    iam_environment_key_ciphertext bytea,
    iam_environment_key_nonce bytea,
    iam_app_secret_ciphertext bytea,
    iam_app_secret_nonce bytea
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    UPDATE briefcase.testing_environments AS environment
       SET last_activity_at = clock_timestamp()
     WHERE environment.root_key_digest = candidate
       AND environment.status = 'active'
    RETURNING environment.org_id,
              environment.environment_id,
              environment.name,
              environment.description,
              environment.key_generation,
              environment.created_at,
              environment.iam_environment_id,
              environment.iam_app_id,
              environment.iam_environment_key_ciphertext,
              environment.iam_environment_key_nonce,
              environment.iam_app_secret_ciphertext,
              environment.iam_app_secret_nonce
$$;

CREATE FUNCTION briefcase.testing_environment_by_iam_digest(candidate bytea)
RETURNS TABLE (
    org_id text,
    environment_id uuid,
    iam_environment_key_ciphertext bytea,
    iam_environment_key_nonce bytea
)
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    UPDATE briefcase.testing_environments AS environment
       SET last_activity_at = clock_timestamp()
     WHERE environment.iam_environment_key_digest = candidate
       AND environment.status = 'active'
    RETURNING environment.org_id,
              environment.environment_id,
              environment.iam_environment_key_ciphertext,
              environment.iam_environment_key_nonce
$$;

CREATE FUNCTION briefcase.active_testing_environment_iam_candidates()
RETURNS TABLE (
    org_id text,
    environment_id uuid,
    iam_environment_key_ciphertext bytea,
    iam_environment_key_nonce bytea
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT environment.org_id,
           environment.environment_id,
           environment.iam_environment_key_ciphertext,
           environment.iam_environment_key_nonce
      FROM briefcase.testing_environments AS environment
     WHERE environment.status = 'active'
     ORDER BY environment.environment_id
$$;

CREATE FUNCTION briefcase.active_testing_environment_count()
RETURNS bigint
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT count(*) FROM briefcase.testing_environments WHERE status = 'active'
$$;

CREATE FUNCTION briefcase.touch_testing_environment(selected uuid)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    WITH touched AS (
        UPDATE briefcase.testing_environments
           SET last_activity_at = clock_timestamp()
         WHERE environment_id = selected AND status = 'active'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM touched)
$$;

REVOKE ALL ON TABLE briefcase.testing_environments FROM PUBLIC;
REVOKE ALL ON TABLE briefcase.testing_environment_idempotency FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.testing_environment_by_root_digest(bytea) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.testing_environment_by_iam_digest(bytea) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.active_testing_environment_iam_candidates() FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.active_testing_environment_count() FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.touch_testing_environment(uuid) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE briefcase.testing_environments TO briefcase_api';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE briefcase.testing_environment_idempotency TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.testing_environment_by_root_digest(bytea) TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.testing_environment_by_iam_digest(bytea) TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.active_testing_environment_iam_candidates() TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.active_testing_environment_count() TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.touch_testing_environment(uuid) TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, UPDATE, DELETE ON TABLE briefcase.testing_environments TO briefcase_worker';
        EXECUTE 'GRANT SELECT, DELETE ON TABLE briefcase.testing_environment_idempotency TO briefcase_worker';
    END IF;
END;
$$;
