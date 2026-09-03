-- Every organization has two bounds: how much it may upload in one UTC day,
-- and how much it may store at once. Both default to a platform-wide value and
-- are configurable per organization by setting the columns below, so an
-- operator raises one organization's ceiling without a deployment:
--
--   INSERT INTO briefcase.organization_usage (org_id, daily_window, storage_limit_bytes)
--   VALUES ('tos', (clock_timestamp() AT TIME ZONE 'UTC')::date, 2251799813685248)
--   ON CONFLICT (org_id) DO UPDATE SET storage_limit_bytes = EXCLUDED.storage_limit_bytes;
--
-- A null limit means the platform default, so the default can be changed later
-- without rewriting every organization that never asked for anything special.
--
-- The counters live in one row per organization, so charging an upload locks
-- exactly what it increments: two uploads racing for the last of an allowance
-- serialize on that row and neither can spend what the other already did.
--
-- `daily_window` is the UTC day the upload counter belongs to. A charge on a
-- later day overwrites the window and restarts the counter, so midnight UTC
-- restores the allowance without a scheduled job.
CREATE TABLE briefcase.organization_usage (
    org_id text NOT NULL,
    daily_window date NOT NULL,
    daily_upload_bytes bigint NOT NULL DEFAULT 0 CHECK (daily_upload_bytes >= 0),
    stored_bytes bigint NOT NULL DEFAULT 0 CHECK (stored_bytes >= 0),
    daily_upload_limit_bytes bigint CHECK (daily_upload_limit_bytes > 0),
    storage_limit_bytes bigint CHECK (storage_limit_bytes > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE
);

CREATE TRIGGER organization_usage_set_updated_at
BEFORE UPDATE ON briefcase.organization_usage
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

-- Stored bytes are whatever the retained versions weigh, so the counter is
-- maintained by the version rows themselves rather than by any one code path.
-- Publication, version retention, bin purges, and cascading entry deletes all
-- move the same number without knowing this counter exists.
CREATE FUNCTION briefcase.track_stored_bytes()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    changed_org text;
    delta bigint;
BEGIN
    IF TG_OP = 'INSERT' THEN
        changed_org := NEW.org_id;
        delta := NEW.size_bytes;
    ELSE
        changed_org := OLD.org_id;
        delta := -OLD.size_bytes;
    END IF;

    INSERT INTO briefcase.organization_usage AS usage_row
        (org_id, daily_window, stored_bytes)
    VALUES (changed_org, (clock_timestamp() AT TIME ZONE 'UTC')::date, GREATEST(delta, 0))
    ON CONFLICT (org_id) DO UPDATE
        SET stored_bytes = GREATEST(usage_row.stored_bytes + delta, 0);

    RETURN NULL;
END;
$$;

CREATE TRIGGER entry_versions_track_stored_bytes
AFTER INSERT OR DELETE ON briefcase.entry_versions
FOR EACH ROW
EXECUTE FUNCTION briefcase.track_stored_bytes();

ALTER TABLE briefcase.organization_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_usage
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

REVOKE ALL ON TABLE briefcase.organization_usage FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.organization_usage TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.organization_usage TO briefcase_worker';
    END IF;
END;
$$;
