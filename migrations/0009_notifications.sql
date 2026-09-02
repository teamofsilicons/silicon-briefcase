-- The central notification inbox.
--
-- Every event that changes what a Carbon or Silicon can reach — a new
-- invitation, a revocation, a request awaiting their decision, or a decision
-- on their own request — is recorded here in the same transaction as the
-- change itself. The inbox therefore cannot disagree with the permissions it
-- describes.
--
-- `entry_id` deliberately carries no foreign key: the notification keeps a
-- name and path snapshot, so the recipient's history survives the eventual
-- permanent purge of the entry it referred to.

CREATE TABLE briefcase.notifications (
    org_id text NOT NULL,
    notification_id uuid NOT NULL,
    recipient_type text NOT NULL CHECK (recipient_type IN ('carbon', 'silicon')),
    recipient_id text NOT NULL,
    kind text NOT NULL CHECK (
        kind IN (
            'access_granted',
            'access_revoked',
            'access_requested',
            'access_request_decided'
        )
    ),
    actor_type text CHECK (actor_type IN ('carbon', 'silicon')),
    actor_id text,
    entry_id uuid,
    details jsonb NOT NULL DEFAULT '{}'::jsonb,
    read_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (org_id, notification_id),
    FOREIGN KEY (org_id)
        REFERENCES briefcase.organizations (org_id)
        ON DELETE CASCADE,
    FOREIGN KEY (org_id, recipient_type, recipient_id)
        REFERENCES briefcase.organization_members (org_id, actor_type, actor_id)
        ON DELETE CASCADE,
    CHECK ((actor_type IS NULL) = (actor_id IS NULL)),
    CHECK (recipient_id = btrim(recipient_id)),
    CHECK (octet_length(recipient_id) BETWEEN 1 AND 255),
    CHECK (actor_id IS NULL OR octet_length(actor_id) BETWEEN 1 AND 255),
    CHECK (jsonb_typeof(details) = 'object'),
    CHECK (pg_column_size(details) <= 8192)
);

-- The inbox is always read newest-first for one recipient.
CREATE INDEX notifications_inbox_idx
    ON briefcase.notifications (
        org_id,
        recipient_type,
        recipient_id,
        created_at DESC,
        notification_id DESC
    );

-- The badge count reads only unread rows.
CREATE INDEX notifications_unread_idx
    ON briefcase.notifications (org_id, recipient_type, recipient_id)
    WHERE read_at IS NULL;

ALTER TABLE briefcase.notifications ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.notifications FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON briefcase.notifications
    USING (org_id = briefcase.current_org_id())
    WITH CHECK (org_id = briefcase.current_org_id());

REVOKE ALL ON TABLE briefcase.notifications FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE ON TABLE briefcase.notifications TO briefcase_api';
    END IF;

    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        EXECUTE 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE briefcase.notifications TO briefcase_worker';
    END IF;
END;
$$;
