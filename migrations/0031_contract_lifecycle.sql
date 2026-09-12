-- API retirement is controlled independently of file versions.
CREATE TABLE briefcase.api_contracts (
 version text PRIMARY KEY,
 status text NOT NULL CHECK (status IN ('active','deprecated','sunset')),
 deprecated_at timestamptz,
 last_request_at timestamptz,
 sunset_at timestamptz,
 CHECK (status <> 'deprecated' OR deprecated_at IS NOT NULL)
);
INSERT INTO briefcase.api_contracts(version,status) VALUES ('v1','active');

-- The same row lock orders a last request against retirement. Active versions
-- need no write contention; exact request times matter only once deprecated.
CREATE FUNCTION briefcase.observe_api_contract(requested text) RETURNS text
LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
DECLARE current_status text;
BEGIN
 SELECT status INTO current_status FROM briefcase.api_contracts WHERE version=requested;
 IF current_status='deprecated' THEN
   UPDATE briefcase.api_contracts SET last_request_at=clock_timestamp()
    WHERE version=requested AND status='deprecated' RETURNING status INTO current_status;
   IF NOT FOUND THEN RETURN 'sunset'; END IF;
 END IF;
 RETURN current_status;
END; $$;
REVOKE ALL ON FUNCTION briefcase.observe_api_contract(text) FROM PUBLIC;
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_api') THEN
   GRANT EXECUTE ON FUNCTION briefcase.observe_api_contract(text) TO briefcase_api;
 END IF;
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_worker') THEN
   GRANT SELECT, UPDATE ON briefcase.api_contracts TO briefcase_worker;
 END IF;
END $$;
