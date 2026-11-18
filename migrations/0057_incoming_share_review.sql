-- Offers are deliberately separate from permission_grants. They cannot
-- participate in entry visibility, inheritance, downloads or invitation history.
CREATE TABLE briefcase.share_reviews (
    grant_id uuid PRIMARY KEY,
    org_id text NOT NULL,
    entry_id uuid NOT NULL,
    entry_name text NOT NULL,
    entry_type text NOT NULL CHECK (entry_type IN ('file','folder')),
    principal_id text NOT NULL REFERENCES briefcase.accounts(account_uuid) ON DELETE CASCADE,
    principal_type text NOT NULL CHECK (principal_type IN ('carbon','silicon')),
    granted_by_id text NOT NULL REFERENCES briefcase.accounts(account_uuid) ON DELETE CASCADE,
    granted_by_type text NOT NULL CHECK (granted_by_type IN ('carbon','silicon')),
    access_mask smallint NOT NULL CHECK (access_mask BETWEEN 1 AND 7),
    inherits_to_descendants boolean NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz,
    FOREIGN KEY (org_id,entry_id) REFERENCES briefcase.entries(org_id,entry_id) ON DELETE CASCADE
);
CREATE INDEX share_reviews_recipient ON briefcase.share_reviews(principal_id,grant_id);
CREATE INDEX share_reviews_entry ON briefcase.share_reviews(org_id,entry_id);
CREATE UNIQUE INDEX share_review_permanent_offer ON briefcase.share_reviews
    (org_id,entry_id,principal_id,granted_by_id) WHERE expires_at IS NULL;

CREATE TABLE briefcase.share_review_senders (
    recipient text NOT NULL REFERENCES briefcase.accounts(account_uuid) ON DELETE CASCADE,
    sender text NOT NULL REFERENCES briefcase.accounts(account_uuid) ON DELETE CASCADE,
    PRIMARY KEY (recipient,sender), CHECK (recipient <> sender)
);

CREATE FUNCTION briefcase.manages_share_review(recipient text) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (SELECT 1 FROM briefcase.accounts a WHERE a.account_uuid=recipient
        AND a.status <> 'deleted' AND current_setting('briefcase.actor_id',true)
        IN (a.account_uuid,a.custodian_uuid))
$$;
ALTER TABLE briefcase.share_reviews ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.share_reviews FORCE ROW LEVEL SECURITY;
CREATE POLICY review_offers ON briefcase.share_reviews
    USING (org_id=briefcase.current_org_id() OR briefcase.manages_share_review(principal_id))
    WITH CHECK (org_id=briefcase.current_org_id() AND granted_by_id=current_setting('briefcase.actor_id',true));
ALTER TABLE briefcase.share_review_senders ENABLE ROW LEVEL SECURITY;
ALTER TABLE briefcase.share_review_senders FORCE ROW LEVEL SECURITY;
CREATE POLICY read_review_senders ON briefcase.share_review_senders FOR SELECT
    USING (sender=current_setting('briefcase.actor_id',true) OR briefcase.manages_share_review(recipient));
CREATE POLICY manage_review_senders ON briefcase.share_review_senders
    USING (briefcase.manages_share_review(recipient)) WITH CHECK (briefcase.manages_share_review(recipient));

-- Binning an entry withdraws its outstanding offers, including on later restore.
CREATE FUNCTION briefcase.withdraw_binned_reviews() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM briefcase.share_reviews WHERE org_id=NEW.org_id AND entry_id=NEW.entry_id;
    RETURN NULL;
END $$;
CREATE TRIGGER withdraw_binned_reviews AFTER UPDATE OF deleted_at ON briefcase.entries
    FOR EACH ROW WHEN (OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL)
    EXECUTE FUNCTION briefcase.withdraw_binned_reviews();
DO $$ BEGIN
    IF EXISTS(SELECT FROM pg_roles WHERE rolname='briefcase_api') THEN
        GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.share_reviews TO briefcase_api;
        GRANT SELECT,INSERT,DELETE ON briefcase.share_review_senders TO briefcase_api;
    END IF;
    IF EXISTS(SELECT FROM pg_roles WHERE rolname='briefcase_worker') THEN
        GRANT SELECT,DELETE ON briefcase.share_reviews TO briefcase_worker;
    END IF;
END $$;

-- Some installations use account-specific runtime roles.
DO $$ BEGIN
    IF EXISTS(SELECT FROM pg_roles WHERE rolname='briefcase_accounts_api') THEN
        GRANT SELECT,INSERT,UPDATE,DELETE ON briefcase.share_reviews TO briefcase_accounts_api;
        GRANT SELECT,INSERT,DELETE ON briefcase.share_review_senders TO briefcase_accounts_api;
    END IF;
    IF EXISTS(SELECT FROM pg_roles WHERE rolname='briefcase_accounts_worker') THEN
        GRANT SELECT,DELETE ON briefcase.share_reviews TO briefcase_accounts_worker;
    END IF;
END $$;

-- Count an accepted offer once in the original sender-to-recipient direction.
CREATE FUNCTION briefcase.remember_accepted_share() RETURNS trigger LANGUAGE plpgsql
SECURITY DEFINER SET search_path=pg_catalog,briefcase AS $$
BEGIN
    INSERT INTO briefcase.invite_history(sender,recipient,invites,last_invited_at)
    VALUES(NEW.metadata->>'sender',NEW.metadata->'principal'->>'id',1,NEW.occurred_at)
    ON CONFLICT(sender,recipient) DO UPDATE SET invites=invite_history.invites+1,
        last_invited_at=greatest(invite_history.last_invited_at,EXCLUDED.last_invited_at);
    RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION briefcase.remember_accepted_share() FROM PUBLIC;
CREATE TRIGGER remember_accepted_share AFTER INSERT ON briefcase.audit_events
    FOR EACH ROW WHEN (NEW.action='permission.share_accepted.v1')
    EXECUTE FUNCTION briefcase.remember_accepted_share();
