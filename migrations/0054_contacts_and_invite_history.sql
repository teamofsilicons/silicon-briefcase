-- Per-account address-book consent and durable, directional invitation counts.
-- History contains no filenames and survives file deletion/audit retention.
CREATE TABLE briefcase.contact_connections (
    account_uuid text PRIMARY KEY REFERENCES briefcase.accounts ON DELETE CASCADE,
    connected_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
ALTER TABLE briefcase.contact_connections ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.contact_connections FORCE ROW LEVEL SECURITY;
CREATE POLICY own_connection ON briefcase.contact_connections
    USING (account_uuid = current_setting('briefcase.actor_id', true))
    WITH CHECK (account_uuid = current_setting('briefcase.actor_id', true));

CREATE TABLE briefcase.invite_history (
    sender text NOT NULL REFERENCES briefcase.accounts ON DELETE CASCADE,
    recipient text NOT NULL REFERENCES briefcase.accounts ON DELETE CASCADE,
    invites bigint NOT NULL CHECK (invites > 0),
    last_invited_at timestamptz NOT NULL,
    PRIMARY KEY (sender, recipient), CHECK (sender <> recipient)
);
CREATE INDEX invite_history_received ON briefcase.invite_history(recipient);
ALTER TABLE briefcase.invite_history ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.invite_history FORCE ROW LEVEL SECURITY;
CREATE POLICY participant_history ON briefcase.invite_history FOR SELECT
    USING (current_setting('briefcase.actor_id', true) IN (sender, recipient));
CREATE POLICY history_writer ON briefcase.invite_history TO CURRENT_USER USING (true) WITH CHECK (true);

-- Only root events count. Ancestor audit copies, retries and failed writes do not.
CREATE FUNCTION briefcase.remember_invite() RETURNS trigger LANGUAGE plpgsql
SECURITY DEFINER SET search_path = pg_catalog, briefcase AS $$
BEGIN
    IF NEW.action IN ('permission.granted.v1', 'permission.expiring_share_granted.v1')
       AND NEW.actor_id <> NEW.metadata->'principal'->>'id'
       AND EXISTS (SELECT 1 FROM briefcase.accounts WHERE account_uuid=NEW.actor_id)
       AND EXISTS (SELECT 1 FROM briefcase.accounts WHERE account_uuid=NEW.metadata->'principal'->>'id') THEN
        INSERT INTO briefcase.invite_history(sender,recipient,invites,last_invited_at)
        VALUES(NEW.actor_id,NEW.metadata->'principal'->>'id',1,NEW.occurred_at)
        ON CONFLICT(sender,recipient) DO UPDATE SET invites=invite_history.invites+1,
            last_invited_at=GREATEST(invite_history.last_invited_at,EXCLUDED.last_invited_at);
    END IF;
    RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION briefcase.remember_invite() FROM PUBLIC;
CREATE TRIGGER remember_invite AFTER INSERT ON briefcase.audit_events
    FOR EACH ROW EXECUTE FUNCTION briefcase.remember_invite();

-- Recover retained history, including revoked/expired shares, using immutable IDs.
CREATE POLICY invite_history_backfill ON briefcase.audit_events FOR SELECT TO CURRENT_USER USING (true);
INSERT INTO briefcase.invite_history(sender,recipient,invites,last_invited_at)
SELECT e.actor_id,e.metadata->'principal'->>'id',count(*),max(e.occurred_at)
FROM briefcase.audit_events e
JOIN briefcase.accounts s ON s.account_uuid=e.actor_id
JOIN briefcase.accounts r ON r.account_uuid=e.metadata->'principal'->>'id'
WHERE e.action IN ('permission.granted.v1','permission.expiring_share_granted.v1')
    AND s.account_uuid<>r.account_uuid
GROUP BY e.actor_id,e.metadata->'principal'->>'id';
DROP POLICY invite_history_backfill ON briefcase.audit_events;

-- Filter actual sharers before pagination, including team/organization grants.
CREATE FUNCTION briefcase.shared_with_people(
    p_type text,p_account text,p_groups text[],p_before timestamptz,p_entry uuid,p_limit integer,
    p_from text,p_contacts text[],p_exclude boolean,p_query text
) RETURNS TABLE(drive text,entry_id uuid,shared_at timestamptz,sharers text[])
LANGUAGE sql STABLE SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
    WITH candidates AS (
        SELECT g.org_id,g.entry_id,g.created_at,g.granted_by_id FROM briefcase.permission_grants g
        WHERE g.principal_type=p_type AND g.principal_id=p_account AND g.revoked_at IS NULL
          AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
        UNION ALL
        SELECT g.org_id,g.entry_id,g.created_at,g.granted_by_id FROM briefcase.group_permission_grants g
        WHERE (g.principal_type||':'||g.principal_id)=ANY(p_groups) AND g.revoked_at IS NULL
          AND (g.expires_at IS NULL OR g.expires_at>clock_timestamp())
    ), shared AS (
        SELECT c.org_id AS drive,c.entry_id,max(c.created_at) AS shared_at,
               array_agg(DISTINCT c.granted_by_id) AS sharers
        FROM candidates c JOIN briefcase.entries e ON (e.org_id,e.entry_id)=(c.org_id,c.entry_id)
        JOIN briefcase.organizations d ON d.org_id=c.org_id AND d.owner_account_uuid=c.org_id
        WHERE e.deleted_at IS NULL AND c.org_id<>p_account
          AND (p_from IS NULL OR c.granted_by_id=p_from)
          AND (p_contacts IS NULL OR ((c.granted_by_id=ANY(p_contacts))<>p_exclude))
          AND (p_query IS NULL OR position(lower(p_query) IN lower(e.name||' '||e.path))>0)
        GROUP BY c.org_id,c.entry_id
    ) SELECT s.drive,s.entry_id,s.shared_at,s.sharers FROM shared s
      WHERE p_before IS NULL OR (s.shared_at,s.entry_id)<(p_before,p_entry)
      ORDER BY s.shared_at DESC,s.entry_id DESC LIMIT LEAST(GREATEST(p_limit,1),101)
$$;
REVOKE ALL ON FUNCTION briefcase.shared_with_people(text,text,text[],timestamptz,uuid,integer,text,text[],boolean,text) FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='briefcase_api') THEN
        GRANT SELECT,INSERT,DELETE ON briefcase.contact_connections TO briefcase_api;
        GRANT SELECT ON briefcase.invite_history TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.shared_with_people(text,text,text[],timestamptz,uuid,integer,text,text[],boolean,text) TO briefcase_api;
    END IF;
END $$;
