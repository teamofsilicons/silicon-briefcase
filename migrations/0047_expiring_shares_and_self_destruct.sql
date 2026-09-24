-- Expiring shares and self-destructing files (UNDERSTANDING.md §Expiring, §Self Destruct).
--
-- An expiring share is its own grant row with an expiry. It never merges into, or
-- replaces, a permanent grant for the same principal: when it expires, only the
-- access it conveyed goes away. Expiry is enforced where grants are read, so a
-- share stops working the instant `expires_at` passes; the worker's later sweep
-- only records the expiry in the logs.

ALTER TABLE briefcase.permission_grants
    ADD COLUMN expires_at timestamptz,
    ADD COLUMN expiry_logged_at timestamptz,
    -- Expiring shares convey read (view/download) and nothing else.
    ADD CONSTRAINT permission_grants_expiring_read_only CHECK (expires_at IS NULL OR access_mask = 1),
    ADD CONSTRAINT permission_grants_expiry_logged CHECK (expiry_logged_at IS NULL OR expires_at IS NOT NULL);

ALTER TABLE briefcase.tag_permission_grants
    ADD COLUMN expires_at timestamptz,
    ADD COLUMN expiry_logged_at timestamptz,
    ADD CONSTRAINT tag_permission_grants_expiring_read_only CHECK (expires_at IS NULL OR access_mask = 1),
    ADD CONSTRAINT tag_permission_grants_expiry_logged CHECK (expiry_logged_at IS NULL OR expires_at IS NOT NULL);

-- One active permanent grant per principal still amends in place; expiring grants
-- sit beside it, one row per share.
DROP INDEX briefcase.permission_grants_active_principal_uidx;
CREATE UNIQUE INDEX permission_grants_active_principal_uidx
    ON briefcase.permission_grants (org_id, entry_id, principal_type, principal_id)
    WHERE revoked_at IS NULL AND expires_at IS NULL;
DROP INDEX briefcase.tag_permission_grants_active_idx;
CREATE UNIQUE INDEX tag_permission_grants_active_idx
    ON briefcase.tag_permission_grants (org_id, entry_id, tag_id)
    WHERE revoked_at IS NULL AND expires_at IS NULL;

CREATE INDEX permission_grants_expiry_sweep_idx
    ON briefcase.permission_grants (expires_at)
    WHERE expires_at IS NOT NULL AND expiry_logged_at IS NULL AND revoked_at IS NULL;
CREATE INDEX tag_permission_grants_expiry_sweep_idx
    ON briefcase.tag_permission_grants (expires_at)
    WHERE expires_at IS NOT NULL AND expiry_logged_at IS NULL AND revoked_at IS NULL;

-- Every access decision reads this view, so an expired share disappears from
-- all of them at once. `expires_at` is appended so existing column positions
-- are unchanged.
CREATE OR REPLACE VIEW briefcase.effective_permission_grants WITH (security_invoker = true) AS
 SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, inherits_to_descendants,
        granted_by_type, granted_by_id, revoked_at, revoked_by_type, revoked_by_id, created_at, expires_at
   FROM briefcase.permission_grants
  WHERE expires_at IS NULL OR expires_at > clock_timestamp()
 UNION ALL
 SELECT g.org_id, g.entry_id, g.grant_id, m.actor_type, m.actor_id, g.access_mask, g.inherits_to_descendants,
        g.granted_by_type, g.granted_by_id, g.revoked_at, g.revoked_by_type, g.revoked_by_id, g.created_at, g.expires_at
   FROM briefcase.tag_permission_grants g
   JOIN briefcase.organization_member_tags t ON t.org_id = g.org_id AND t.tag_id = g.tag_id
   JOIN briefcase.organization_tags tag ON tag.org_id = t.org_id AND tag.tag_id = t.tag_id AND tag.lifecycle_status = 'active'
   JOIN briefcase.organization_members m ON m.org_id = t.org_id AND m.actor_type = t.actor_type AND m.actor_id = t.actor_id AND m.membership_status = 'active'
  WHERE g.expires_at IS NULL OR g.expires_at > clock_timestamp();

-- "Anyone with the link" is one flag per entry, so an expiring link is that flag
-- with an expiry. A permanent link has no expiry.
ALTER TABLE briefcase.entries
    ADD COLUMN link_expires_at timestamptz,
    ADD CONSTRAINT entries_link_expiry_needs_link CHECK (link_expires_at IS NULL OR link_public);
CREATE INDEX entries_link_expiry_sweep_idx
    ON briefcase.entries (link_expires_at)
    WHERE link_expires_at IS NOT NULL;

-- Self destruct: set only when a new file is uploaded, counted from the moment
-- the upload finishes. Clearing it makes the file permanent.
ALTER TABLE briefcase.entries
    ADD COLUMN self_destruct_at timestamptz,
    ADD CONSTRAINT entries_self_destruct_files_only CHECK (self_destruct_at IS NULL OR entry_type = 'file');
CREATE INDEX entries_self_destruct_sweep_idx
    ON briefcase.entries (self_destruct_at)
    WHERE self_destruct_at IS NOT NULL AND deleted_at IS NULL;

-- A large upload is published when its multipart session completes, so the
-- requested lifetime waits on the session until then.
ALTER TABLE briefcase.multipart_uploads
    ADD COLUMN self_destruct_minutes integer
        CHECK (self_destruct_minutes IS NULL OR self_destruct_minutes BETWEEN 1 AND 43200);
