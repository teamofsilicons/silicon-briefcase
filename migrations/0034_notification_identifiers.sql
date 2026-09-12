-- PostgreSQL 16 has no built-in uuidv7 generator. Set-based notification fanout
-- uses the same RFC 9562 UUID version required by the Rust NotificationId type.
CREATE FUNCTION briefcase.new_uuid_v7() RETURNS uuid
LANGUAGE plpgsql VOLATILE SET search_path=pg_catalog AS $$
DECLARE
    value bytea := uuid_send(gen_random_uuid());
    millis bigint := floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint;
BEGIN
    value := overlay(value PLACING substring(int8send(millis) FROM 3 FOR 6) FROM 1 FOR 6);
    value := set_byte(value, 6, (get_byte(value, 6) & 15) | 112);
    -- gen_random_uuid already supplies random bits and the RFC variant bits.
    RETURN encode(value, 'hex')::uuid;
END; $$;
REVOKE ALL ON FUNCTION briefcase.new_uuid_v7() FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_api') THEN
        GRANT EXECUTE ON FUNCTION briefcase.new_uuid_v7() TO briefcase_api;
    END IF;
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_worker') THEN
        GRANT EXECUTE ON FUNCTION briefcase.new_uuid_v7() TO briefcase_worker;
    END IF;
END $$;
