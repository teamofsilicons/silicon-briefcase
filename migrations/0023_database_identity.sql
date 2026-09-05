-- A sandbox database must never resolve to the production database, even when
-- its connection string uses another hostname, role, password, query ordering,
-- or URI spelling. Expose only the immutable cluster/database identity needed
-- by startup; runtime roles do not receive general pg_control access.

CREATE FUNCTION briefcase.database_identity()
RETURNS TABLE (
    system_identifier text,
    database_oid bigint
)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, pg_temp
AS $function$
    SELECT control.system_identifier::text,
           database.oid::bigint
      FROM pg_catalog.pg_control_system() AS control
      JOIN pg_catalog.pg_database AS database
        ON database.datname = pg_catalog.current_database()
$function$;

REVOKE ALL ON FUNCTION briefcase.database_identity() FROM PUBLIC;

DO $block$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.database_identity() TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT EXECUTE ON FUNCTION briefcase.database_identity() TO briefcase_worker';
    END IF;
END
$block$;
