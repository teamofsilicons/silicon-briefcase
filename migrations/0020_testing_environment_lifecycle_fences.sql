-- Activity is accepted only for the exact control generation authenticated by
-- the caller. UUID-only touches could otherwise keep a replacement pairing
-- alive, or let a webhook signed by the prior IAM root cross a concurrent
-- re-pair.
DROP FUNCTION briefcase.touch_testing_environment(uuid);

CREATE FUNCTION briefcase.touch_testing_environment(selected uuid, expected_version bigint)
RETURNS boolean
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    WITH touched AS (
        UPDATE briefcase.testing_environments
           SET last_activity_at = clock_timestamp()
         WHERE environment_id = selected
           AND status = 'active'
           AND version = expected_version
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM touched)
$$;

DROP FUNCTION briefcase.touch_testing_environment_generation(uuid);

CREATE FUNCTION briefcase.touch_testing_environment_generation(
    selected uuid,
    expected_version bigint
)
RETURNS bigint
LANGUAGE sql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    UPDATE briefcase.testing_environments
       SET last_activity_at = clock_timestamp()
     WHERE environment_id = selected
       AND status = 'active'
       AND version = expected_version
    RETURNING version
$$;

REVOKE ALL ON FUNCTION briefcase.touch_testing_environment(uuid, bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.touch_testing_environment_generation(uuid, bigint) FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.touch_testing_environment(uuid, bigint) TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.touch_testing_environment_generation(uuid, bigint) TO briefcase_api';
    END IF;
END;
$$;

-- Purge first claims an expired environment into a durable, non-restorable
-- state. A crash can safely resume this state, while restore can no longer win
-- between provider/database erasure and deletion of the control record.
ALTER TABLE briefcase.testing_environments
    DROP CONSTRAINT testing_environments_status_check,
    DROP CONSTRAINT testing_environments_check;

ALTER TABLE briefcase.testing_environments
    ADD CONSTRAINT testing_environments_status_check
        CHECK (status IN ('active', 'deleted', 'purging')),
    ADD CONSTRAINT testing_environments_key_state_check CHECK (
        (status = 'active'
            AND root_key_digest IS NOT NULL
            AND octet_length(root_key_digest) = 32
            AND root_key_ciphertext IS NOT NULL
            AND root_key_nonce IS NOT NULL
            AND octet_length(root_key_nonce) = 12
            AND deleted_at IS NULL
            AND purge_after IS NULL)
        OR
        (status IN ('deleted', 'purging')
            AND root_key_digest IS NULL
            AND root_key_ciphertext IS NULL
            AND root_key_nonce IS NULL
            AND deleted_at IS NOT NULL
            AND purge_after IS NOT NULL
            AND purge_after >= deleted_at)
    );
