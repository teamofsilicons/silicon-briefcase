-- Silicon Accounts identities. Accounts are personal (a Carbon or a Silicon)
-- and keyed by their permanent, case-sensitive uuid (`zQo`), never by the
-- public id, which can change. Nothing here replaces or deletes the legacy
-- organization tables; they stay readable for the cutover re-keying.

CREATE TABLE briefcase.accounts (
    account_uuid text COLLATE "C" PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('carbon', 'silicon')),
    -- Current public id (`c:ada`, `si:scout`), for display and by-id resolution.
    public_id text COLLATE "C",
    -- When `public_id` was last set, so an older sighting never overwrites a newer one.
    public_id_observed_at timestamptz,
    display_name text,
    pfp_url text,
    -- Only when the account shared its email with Briefcase.
    email text,
    -- A Silicon's custodian (a Carbon); NULL for Carbons or while unknown.
    custodian_uuid text COLLATE "C",
    custodian_observed_at timestamptz,
    -- Monotonic Silicon Accounts profile version (orders account.updated).
    profile_version bigint NOT NULL DEFAULT 0 CHECK (profile_version >= 0),
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'access_removed', 'deleted')),
    -- Access tokens issued at or before this instant are refused.
    revoked_before timestamptz,
    deleted_at timestamptz,
    -- Last time Silicon Accounts confirmed this row (sign-in, lookup, webhook).
    refreshed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (account_uuid ~ '^[A-Za-z0-9]{3,32}$'),
    CHECK (custodian_uuid IS NULL OR custodian_uuid ~ '^[A-Za-z0-9]{3,32}$'),
    CHECK (custodian_uuid IS NULL OR kind = 'silicon'),
    CHECK (public_id IS NULL OR public_id ~ '^(c|si):[a-z0-9_-]{1,64}$'),
    CHECK (display_name IS NULL OR octet_length(display_name) <= 512),
    CHECK (pfp_url IS NULL OR octet_length(pfp_url) <= 2048),
    CHECK (email IS NULL OR octet_length(email) BETWEEN 3 AND 320),
    CHECK ((status = 'deleted') = (deleted_at IS NOT NULL))
);

CREATE INDEX accounts_public_id_idx
    ON briefcase.accounts (public_id)
    WHERE public_id IS NOT NULL;

CREATE INDEX accounts_custodian_idx
    ON briefcase.accounts (custodian_uuid, account_uuid)
    WHERE custodian_uuid IS NOT NULL;

CREATE TRIGGER accounts_set_updated_at
BEFORE UPDATE ON briefcase.accounts
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

-- Every public id an account has had, so links and names using an old id can
-- still be resolved (account.id_changed).
CREATE TABLE briefcase.account_id_history (
    public_id text COLLATE "C" NOT NULL,
    account_uuid text COLLATE "C" NOT NULL
        REFERENCES briefcase.accounts (account_uuid) ON DELETE CASCADE,
    -- When the account stopped using this id.
    replaced_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (public_id, account_uuid),
    CHECK (public_id ~ '^(c|si):[a-z0-9_-]{1,64}$')
);

CREATE INDEX account_id_history_account_idx
    ON briefcase.account_id_history (account_uuid, replaced_at DESC);

-- Silicon Accounts webhook deliveries, deduplicated on event_id (retries and
-- replays reuse it). Payloads are not stored; only what was done.
CREATE TABLE briefcase.accounts_webhook_receipts (
    event_id text COLLATE "C" PRIMARY KEY,
    event_type text NOT NULL,
    account_uuid text COLLATE "C",
    occurred_at timestamptz,
    outcome text NOT NULL CHECK (outcome IN ('applied', 'stale', 'ignored')),
    received_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (octet_length(event_id) BETWEEN 1 AND 255),
    CHECK (octet_length(event_type) BETWEEN 1 AND 255)
);

CREATE INDEX accounts_webhook_receipts_account_idx
    ON briefcase.accounts_webhook_receipts (account_uuid, received_at DESC)
    WHERE account_uuid IS NOT NULL;

REVOKE ALL ON briefcase.accounts FROM PUBLIC;
REVOKE ALL ON briefcase.account_id_history FROM PUBLIC;
REVOKE ALL ON briefcase.accounts_webhook_receipts FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT SELECT, INSERT, UPDATE ON briefcase.accounts TO briefcase_api;
        GRANT SELECT, INSERT, UPDATE ON briefcase.account_id_history TO briefcase_api;
        GRANT SELECT, INSERT ON briefcase.accounts_webhook_receipts TO briefcase_api;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        GRANT SELECT, UPDATE ON briefcase.accounts TO briefcase_worker;
        GRANT SELECT ON briefcase.account_id_history TO briefcase_worker;
        GRANT SELECT, DELETE ON briefcase.accounts_webhook_receipts TO briefcase_worker;
    END IF;
END;
$$;
