-- Accept canonical 128-bit account UUIDs alongside legacy identifiers.
-- Existing keys remain unchanged here; the explicit mapping consumer performs re-keying.

ALTER TABLE briefcase.accounts DROP CONSTRAINT accounts_account_uuid_check;
ALTER TABLE briefcase.accounts ADD CONSTRAINT accounts_account_uuid_check
    CHECK (account_uuid ~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$');

ALTER TABLE briefcase.accounts DROP CONSTRAINT accounts_custodian_uuid_check;
ALTER TABLE briefcase.accounts ADD CONSTRAINT accounts_custodian_uuid_check
    CHECK (custodian_uuid IS NULL OR custodian_uuid ~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$');

ALTER TABLE briefcase.organizations DROP CONSTRAINT organizations_owner_account_uuid_check;
ALTER TABLE briefcase.organizations ADD CONSTRAINT organizations_owner_account_uuid_check
    CHECK (owner_account_uuid IS NULL OR owner_account_uuid ~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$');

ALTER TABLE briefcase.silicon_share_allowances DROP CONSTRAINT silicon_share_allowances_allowed_by_uuid_check;
ALTER TABLE briefcase.silicon_share_allowances ADD CONSTRAINT silicon_share_allowances_allowed_by_uuid_check
    CHECK (allowed_by_uuid ~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$');

ALTER TABLE briefcase.identity_links DROP CONSTRAINT identity_links_accounts_uuid_check;
ALTER TABLE briefcase.identity_links ADD CONSTRAINT identity_links_accounts_uuid_check
    CHECK (accounts_uuid ~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$');

CREATE OR REPLACE FUNCTION briefcase.deliver_notification(
    p_recipient_type text,
    p_recipient_id text,
    p_kind text,
    p_actor_type text,
    p_actor_id text,
    p_entry_id uuid,
    p_details jsonb
)
RETURNS boolean
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    original text := COALESCE(current_setting('briefcase.org_id', true), '');
    recorded_owner text;
BEGIN
    IF p_recipient_type IS NULL OR p_recipient_type NOT IN ('carbon', 'silicon')
        OR p_recipient_id IS NULL OR p_recipient_id !~ '^([A-Za-z0-9]{3,32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$'
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '22023',
            MESSAGE = 'a notification recipient is a Silicon Accounts account';
    END IF;
    PERFORM set_config('briefcase.org_id', p_recipient_id, true);
    INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status, owner_account_uuid)
    VALUES (p_recipient_id, 0, 'active', p_recipient_id)
    ON CONFLICT (org_id) DO NOTHING;
    SELECT organization.owner_account_uuid
      INTO recorded_owner
      FROM briefcase.organizations AS organization
     WHERE organization.org_id = p_recipient_id;
    IF recorded_owner IS DISTINCT FROM p_recipient_id THEN
        PERFORM set_config('briefcase.org_id', original, true);
        RETURN false;
    END IF;
    INSERT INTO briefcase.organization_members
        (org_id, actor_type, actor_id, org_role, membership_status, iam_version)
    VALUES (p_recipient_id, p_recipient_type, p_recipient_id, 'owner', 'active', 0)
    ON CONFLICT (org_id, actor_type, actor_id) DO NOTHING;
    INSERT INTO briefcase.notifications (
        org_id, notification_id, recipient_type, recipient_id, kind,
        actor_type, actor_id, entry_id, details
    ) VALUES (
        p_recipient_id, briefcase.new_uuid_v7(), p_recipient_type, p_recipient_id, p_kind,
        p_actor_type, p_actor_id, p_entry_id, COALESCE(p_details, '{}'::jsonb)
    );
    PERFORM set_config('briefcase.org_id', original, true);
    RETURN true;
END;
$$;
