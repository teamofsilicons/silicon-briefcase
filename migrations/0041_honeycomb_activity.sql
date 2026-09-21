-- Bounded, secret-encrypted activity outbox for the service reporter.
CREATE FUNCTION briefcase.honeycomb_activity_outbox()
RETURNS TABLE(environment_id uuid,org_id text,generation bigint,key_version bigint,key_ciphertext bytea,key_nonce bytea,activity_at timestamptz)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT environment_id,org_id,generation,key_version,testing_key_ciphertext,testing_key_nonce,last_activity_at
 FROM briefcase.honeycomb_environments WHERE state='active' AND last_activity_at IS NOT NULL
 AND (activity_reported_at IS NULL OR last_activity_at>activity_reported_at)
 ORDER BY last_activity_at LIMIT 10;
$$;
REVOKE ALL ON FUNCTION briefcase.honeycomb_activity_outbox() FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.honeycomb_activity_outbox() TO briefcase_api;
END IF; END $$;
