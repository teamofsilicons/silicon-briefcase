-- Immutable record of the Accounts-wide UUID migration. These are tombstones,
-- never login aliases: a credential naming old_uuid must sign in again.
CREATE TABLE briefcase.account_uuid_mappings (
    old_uuid text COLLATE "C" PRIMARY KEY,
    new_uuid text COLLATE "C" NOT NULL UNIQUE,
    kind text NOT NULL CHECK (kind IN ('carbon', 'silicon')),
    mapping_sha256 text NOT NULL CHECK (mapping_sha256 ~ '^[0-9a-f]{64}$'),
    migrated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (old_uuid <> new_uuid),
    CHECK (new_uuid ~ '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$')
);
REVOKE ALL ON briefcase.account_uuid_mappings FROM PUBLIC;

CREATE FUNCTION briefcase.prevent_account_mapping_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'account UUID mappings are immutable';
END;
$$;
CREATE TRIGGER account_uuid_mappings_immutable
    BEFORE UPDATE OR DELETE ON briefcase.account_uuid_mappings
    FOR EACH ROW EXECUTE FUNCTION briefcase.prevent_account_mapping_mutation();

-- Historical payloads may contain signed or deduplicated identity bytes. Keep
-- those bytes, but permanently seal their dispatch state at the cutover.
ALTER TABLE briefcase.outbox_events ADD COLUMN account_uuid_retired boolean NOT NULL DEFAULT false;
CREATE FUNCTION briefcase.prevent_retired_outbox_replay()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (TG_OP = 'UPDATE' AND OLD.account_uuid_retired AND NOT NEW.account_uuid_retired)
       OR (NEW.account_uuid_retired AND NEW.status IN ('pending','processing')) THEN
        RAISE EXCEPTION 'historical account UUID delivery cannot be replayed';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER outbox_reject_retired_account_replay
    BEFORE INSERT OR UPDATE ON briefcase.outbox_events
    FOR EACH ROW EXECUTE FUNCTION briefcase.prevent_retired_outbox_replay();

CREATE FUNCTION briefcase.prevent_retired_account_uuid()
RETURNS trigger LANGUAGE plpgsql SET search_path = pg_catalog, briefcase AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM briefcase.account_uuid_mappings WHERE old_uuid = NEW.account_uuid
               OR old_uuid = NEW.custodian_uuid) THEN
        RAISE EXCEPTION 'account UUID was migrated; refresh the Accounts identity'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER accounts_reject_retired_uuid
    BEFORE INSERT OR UPDATE OF account_uuid, custodian_uuid ON briefcase.accounts
    FOR EACH ROW EXECUTE FUNCTION briefcase.prevent_retired_account_uuid();

DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT SELECT ON briefcase.account_uuid_mappings TO briefcase_api;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'briefcase_worker') THEN
        GRANT SELECT ON briefcase.account_uuid_mappings TO briefcase_worker;
    END IF;
END $$;
