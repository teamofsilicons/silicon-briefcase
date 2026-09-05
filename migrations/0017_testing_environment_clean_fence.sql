-- A root key authenticates both an environment and the control-plane version
-- observed at lookup time. Every test-plane transaction revalidates that
-- version after acquiring its shared clean fence. A cleaner advances the
-- version while holding the exclusive fence, preventing a request that waited
-- behind the clean from publishing into the newly emptied data plane.

DROP FUNCTION briefcase.testing_environment_by_root_digest(bytea);

CREATE FUNCTION briefcase.testing_environment_by_root_digest(candidate bytea)
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
              environment.version,
              environment.created_at,
              environment.iam_environment_id,
              environment.iam_app_id,
              environment.iam_environment_key_ciphertext,
              environment.iam_environment_key_nonce,
              environment.iam_app_secret_ciphertext,
              environment.iam_app_secret_nonce
$$;

DROP FUNCTION briefcase.active_testing_environment_iam_candidates();

CREATE FUNCTION briefcase.active_testing_environment_iam_candidates()
RETURNS TABLE (
    org_id text,
    environment_id uuid,
    control_version bigint,
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
           environment.version,
           environment.iam_environment_key_ciphertext,
           environment.iam_environment_key_nonce
      FROM briefcase.testing_environments AS environment
     WHERE environment.status = 'active'
     ORDER BY environment.environment_id
$$;

-- The candidate scan and constant-time key comparison happen in application
-- code. Touching the matching row returns its current generation atomically,
-- rather than trusting the possibly stale version from the candidate scan.
CREATE FUNCTION briefcase.touch_testing_environment_generation(selected uuid)
RETURNS bigint
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    UPDATE briefcase.testing_environments
       SET last_activity_at = clock_timestamp()
     WHERE environment_id = selected AND status = 'active'
    RETURNING version
$$;

CREATE FUNCTION briefcase.testing_environment_version_matches(
    selected uuid,
    expected_version bigint
)
RETURNS boolean
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT EXISTS (
        SELECT 1
          FROM briefcase.testing_environments
         WHERE environment_id = selected
           AND status = 'active'
           AND version = expected_version
    )
$$;

REVOKE ALL ON FUNCTION briefcase.testing_environment_by_root_digest(bytea) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.active_testing_environment_iam_candidates() FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.touch_testing_environment_generation(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.testing_environment_version_matches(uuid, bigint) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.testing_environment_by_root_digest(bytea) TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.active_testing_environment_iam_candidates() TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.touch_testing_environment_generation(uuid) TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.testing_environment_version_matches(uuid, bigint) TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.testing_environment_version_matches(uuid, bigint) TO briefcase_worker';
    END IF;
END;
$$;
