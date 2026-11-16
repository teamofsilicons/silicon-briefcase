-- Organizations owns membership; Briefcase stores only the group recipient.
-- A request installs its freshly verified memberships in a transaction-local
-- setting. No cached roster, former member or recycled team name grants access.
CREATE TABLE briefcase.group_permission_grants (
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    grant_id uuid NOT NULL,
    principal_type text NOT NULL CHECK (principal_type IN ('organization', 'team')),
    principal_id text NOT NULL CHECK (length(principal_id) BETWEEN 1 AND 180),
    display_name text NOT NULL,
    access_mask smallint NOT NULL CHECK (access_mask BETWEEN 1 AND 7 AND (access_mask & 1) = 1),
    inherits_to_descendants boolean NOT NULL,
    granted_by_type text NOT NULL CHECK (granted_by_type IN ('carbon', 'silicon')),
    granted_by_id text NOT NULL,
    revoked_at timestamptz,
    revoked_by_type text,
    revoked_by_id text,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expiry_logged_at timestamptz,
    expires_at timestamptz CHECK (expires_at IS NULL OR access_mask = 1),
    PRIMARY KEY (org_id, grant_id),
    FOREIGN KEY (org_id, entry_id) REFERENCES briefcase.entries(org_id, entry_id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX group_grants_permanent ON briefcase.group_permission_grants(org_id, entry_id, principal_type, principal_id)
    WHERE revoked_at IS NULL AND expires_at IS NULL;
CREATE INDEX group_grants_recipient ON briefcase.group_permission_grants(principal_type, principal_id, created_at DESC)
    WHERE revoked_at IS NULL;
CREATE INDEX group_grants_expiry ON briefcase.group_permission_grants(expires_at) WHERE expires_at IS NOT NULL AND expiry_logged_at IS NULL AND revoked_at IS NULL;
ALTER TABLE briefcase.group_permission_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.group_permission_grants FORCE ROW LEVEL SECURITY;
CREATE POLICY group_grants_tenant ON briefcase.group_permission_grants
    USING (org_id = briefcase.current_org_id()) WITH CHECK (org_id = briefcase.current_org_id());

-- Only the migration owner used by the narrow discovery functions may read across drives.
CREATE POLICY drive_definer_lookup ON briefcase.group_permission_grants
    FOR SELECT TO CURRENT_USER USING (true);

-- Retire the old IAM-tag authority. Historical rows stay intact for auditing.
CREATE OR REPLACE VIEW briefcase.effective_permission_grants WITH (security_invoker = true) AS
 SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, inherits_to_descendants,
        granted_by_type, granted_by_id, revoked_at, revoked_by_type, revoked_by_id, created_at, expires_at
   FROM briefcase.permission_grants
  WHERE expires_at IS NULL OR expires_at > clock_timestamp()
 UNION ALL
 SELECT g.org_id, g.entry_id, g.grant_id,
        current_setting('briefcase.actor_type', true), current_setting('briefcase.actor_id', true),
        g.access_mask, g.inherits_to_descendants, g.granted_by_type, g.granted_by_id,
        g.revoked_at, g.revoked_by_type, g.revoked_by_id, g.created_at, g.expires_at
   FROM briefcase.group_permission_grants g
  WHERE COALESCE(NULLIF(current_setting('briefcase.groups', true), ''), '[]')::jsonb ? (g.principal_type || ':' || g.principal_id)
    AND (g.expires_at IS NULL OR g.expires_at > clock_timestamp());

-- Discovery only returns entry locators. Normal drive RLS and file policy
-- still authorize each result. Groups are supplied by verified API context.
CREATE FUNCTION briefcase.shared_with_groups(
    p_type text, p_account text, p_groups text[], p_before timestamptz, p_entry uuid, p_limit integer
) RETURNS TABLE (drive text, entry_id uuid, shared_at timestamptz)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, briefcase AS $$
    WITH candidates AS (
        SELECT g.org_id, g.entry_id, g.created_at
          FROM briefcase.permission_grants g
         WHERE g.principal_type=p_type AND g.principal_id=p_account AND g.revoked_at IS NULL
           AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
        UNION ALL
        SELECT g.org_id, g.entry_id, g.created_at
          FROM briefcase.group_permission_grants g
         WHERE (g.principal_type || ':' || g.principal_id) = ANY(p_groups) AND g.revoked_at IS NULL
           AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
    ), shared AS (
        SELECT c.org_id AS drive, c.entry_id, max(c.created_at) AS shared_at
          FROM candidates c JOIN briefcase.entries e ON (e.org_id,e.entry_id)=(c.org_id,c.entry_id)
          JOIN briefcase.organizations d ON d.org_id=c.org_id AND d.owner_account_uuid=c.org_id
         WHERE e.deleted_at IS NULL AND c.org_id<>p_account GROUP BY c.org_id,c.entry_id
    ) SELECT s.drive,s.entry_id,s.shared_at FROM shared s
       WHERE p_before IS NULL OR (s.shared_at,s.entry_id)<(p_before,p_entry)
       ORDER BY s.shared_at DESC,s.entry_id DESC LIMIT LEAST(GREATEST(p_limit,1),101)
$$;
CREATE FUNCTION briefcase.group_shared_drives(p_type text, p_account text, p_groups text[], p_limit integer)
RETURNS TABLE (drive text) LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, briefcase AS $$
    WITH candidates AS (
        SELECT g.org_id, g.created_at FROM briefcase.permission_grants g
         WHERE g.principal_type=p_type AND g.principal_id=p_account AND g.revoked_at IS NULL
           AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
        UNION ALL
        SELECT g.org_id, g.created_at FROM briefcase.group_permission_grants g
         WHERE (g.principal_type || ':' || g.principal_id)=ANY(p_groups) AND g.revoked_at IS NULL
           AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
    ) SELECT c.org_id FROM candidates c JOIN briefcase.organizations d
         ON d.org_id=c.org_id AND d.owner_account_uuid=c.org_id
       WHERE c.org_id<>p_account GROUP BY c.org_id ORDER BY max(c.created_at) DESC,c.org_id
       LIMIT LEAST(GREATEST(p_limit,1),1000)
$$;
REVOKE ALL ON FUNCTION briefcase.shared_with_groups(text,text,text[],timestamptz,uuid,integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.group_shared_drives(text,text,text[],integer) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_api') THEN
        GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.group_permission_grants TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.shared_with_groups(text,text,text[],timestamptz,uuid,integer) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.group_shared_drives(text,text,text[],integer) TO briefcase_api;
    END IF;
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_worker') THEN
        GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.group_permission_grants TO briefcase_worker;
    END IF;
END $$;
