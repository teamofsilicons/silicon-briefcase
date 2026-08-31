REVOKE ALL ON SCHEMA briefcase FROM PUBLIC;
REVOKE ALL ON ALL TABLES IN SCHEMA briefcase FROM PUBLIC;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA briefcase FROM PUBLIC;
REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA briefcase FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT USAGE ON SCHEMA briefcase TO briefcase_api';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA briefcase TO briefcase_api';
        EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA briefcase TO briefcase_api';
        EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA briefcase TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT USAGE ON SCHEMA briefcase TO briefcase_worker';
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA briefcase TO briefcase_worker';
        EXECUTE 'GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA briefcase TO briefcase_worker';
        EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA briefcase TO briefcase_worker';
    END IF;
END;
$$;
