-- A fresh IAM response must be bound to the participant revision observed before
-- that request. IAM and Honeycomb revisions remain separate counters.
CREATE FUNCTION briefcase.honeycomb_discovery_revision(selected uuid)
RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
 SELECT environment_revision FROM briefcase.honeycomb_environments WHERE environment_id=selected;
$$;
REVOKE ALL ON FUNCTION briefcase.honeycomb_discovery_revision(uuid) FROM PUBLIC;
DO $$ BEGIN IF to_regrole('briefcase_api') IS NOT NULL THEN
 GRANT EXECUTE ON FUNCTION briefcase.honeycomb_discovery_revision(uuid) TO briefcase_api;
END IF; END $$;
