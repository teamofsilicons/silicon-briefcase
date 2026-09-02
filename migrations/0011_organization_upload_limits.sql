-- Every organization has a daily upload allowance and a total one.
--
-- The counters live in one row per organization so a charge is a single
-- upsert that locks exactly what it increments: two concurrent uploads
-- serialize on that row, and neither can admit bytes the other already spent.
--
-- `daily_window` is the UTC day the daily counter belongs to. A charge on a
-- later day overwrites the window and restarts its counter, so midnight UTC
-- resets the allowance without a scheduled job. Both counters measure uploaded
-- bytes rather than stored bytes: deleting a file frees storage, not
-- allowance.
CREATE TABLE briefcase.organization_upload_usage (
    org_id text NOT NULL,
    daily_window date NOT NULL,
    daily_bytes bigint NOT NULL DEFAULT 0 CHECK (daily_bytes >= 0),
    total_bytes bigint NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE
);

CREATE TRIGGER organization_upload_usage_set_updated_at
BEFORE UPDATE ON briefcase.organization_upload_usage
FOR EACH ROW
EXECUTE FUNCTION briefcase.set_updated_at();

ALTER TABLE briefcase.organization_upload_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_upload_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.organization_upload_usage
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

REVOKE ALL ON TABLE briefcase.organization_upload_usage FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.organization_upload_usage TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.organization_upload_usage TO briefcase_worker';
    END IF;
END;
$$;
