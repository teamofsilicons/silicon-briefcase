-- Personal drives replace organizations.
--
-- Every Silicon Accounts account (Carbon or Silicon) owns one drive. The drive
-- is the storage tenant: the physical `org_id` column keeps its name and now
-- holds the owner's account uuid, so every composite key, foreign key, row
-- security policy, usage row, storage configuration and bin partition carries
-- over unchanged. A drive is an `organizations` row whose
-- `owner_account_uuid` equals its key; pre-migration organizations keep NULL
-- and stay untouched until the cutover re-keys them. Nothing here deletes or
-- rewrites existing content.

-- A drive has exactly one root folder. Its children are the top of the
-- account's files, so their paths start fresh below it (`notes/a.md`, not
-- `{root}/notes/a.md`) and permanent URLs read `/{owner}/{path}`. The root's
-- own path is `.`, which no file or folder name can ever produce.
ALTER TABLE briefcase.entries DROP CONSTRAINT entries_system_kind_check;
ALTER TABLE briefcase.entries ADD CONSTRAINT entries_system_kind_check CHECK (
    system_kind IS NULL OR system_kind IN (
        'public_root', 'private_root', 'tag_root', 'actor_root', 'app_container',
        'apps_root', 'app_root', 'app_public', 'app_private', 'app_actor',
        'drive_root'
    )
);
ALTER TABLE briefcase.entries ADD CONSTRAINT entries_drive_root_shape CHECK (
    system_kind IS DISTINCT FROM 'drive_root'
    OR (parent_id IS NULL AND root_type = 'private' AND tag_id IS NULL AND entry_type = 'folder')
);
CREATE UNIQUE INDEX entries_drive_root_uidx
    ON briefcase.entries (org_id)
    WHERE system_kind = 'drive_root';

-- Entries are addressed by identifier across drives (a shared file is opened
-- by its id), so the identifier needs its own index.
CREATE INDEX entries_entry_id_idx ON briefcase.entries (entry_id);

CREATE OR REPLACE FUNCTION briefcase.derive_entry_path()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    parent_path text;
    parent_kind text;
BEGIN
    IF TG_OP = 'UPDATE'
        AND NEW.name = OLD.name
        AND NEW.parent_id IS NOT DISTINCT FROM OLD.parent_id
    THEN
        RETURN NEW;
    END IF;

    IF NEW.system_kind = 'drive_root' THEN
        NEW.path = '.';
        RETURN NEW;
    END IF;

    IF NEW.parent_id IS NULL THEN
        NEW.path = NEW.name;
        RETURN NEW;
    END IF;

    -- The share lock serializes this derivation against a concurrent rename
    -- or move of the parent, which relocates the subtree by prefix rewrite.
    SELECT parent.path, parent.system_kind
      INTO parent_path, parent_kind
      FROM briefcase.entries AS parent
     WHERE parent.org_id = NEW.org_id
       AND parent.entry_id = NEW.parent_id
       FOR SHARE;

    IF parent_path IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23503',
            MESSAGE = 'entry parent does not exist in the organization';
    END IF;

    IF parent_kind = 'drive_root' THEN
        NEW.path = NEW.name;
    ELSE
        NEW.path = parent_path || '/' || NEW.name;
    END IF;
    RETURN NEW;
END;
$$;

-- `apps` at the top of a drive is the container apps write into; delegated
-- access is confined by that exact path, so nothing else may take the name.
CREATE FUNCTION briefcase.reserve_drive_root_names()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF NEW.name = 'apps'
        AND NEW.system_kind IS DISTINCT FROM 'apps_root'
        AND NEW.parent_id IS NOT NULL
        AND NEW.deleted_at IS NULL
        AND EXISTS (
            SELECT 1 FROM briefcase.entries AS parent
             WHERE parent.org_id = NEW.org_id
               AND parent.entry_id = NEW.parent_id
               AND parent.system_kind = 'drive_root'
        )
    THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'apps at the top of a drive is kept for apps';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER entries_reserve_drive_root_names
BEFORE INSERT OR UPDATE OF name, parent_id, deleted_at ON briefcase.entries
FOR EACH ROW
EXECUTE FUNCTION briefcase.reserve_drive_root_names();

-- A drive's key is its owner's uuid.
ALTER TABLE briefcase.organizations
    ADD CONSTRAINT organizations_drive_key_is_owner
        CHECK (owner_account_uuid IS NULL OR owner_account_uuid = org_id);

-- Bring-your-own buckets: the STS External ID a customer's trust policy names
-- is persisted with the configuration, so it survives the move from
-- organization handles to drive uuids. Existing configurations keep exactly
-- the id they were derived with (SHA-256 over a fixed prefix and the
-- organization handle); new ones are derived the same way from the drive key.
ALTER TABLE briefcase.organization_storage_configs ADD COLUMN external_id text;
ALTER TABLE briefcase.organization_storage_configs NO FORCE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_storage_configs
    DISABLE TRIGGER organization_storage_configs_set_updated_at;
UPDATE briefcase.organization_storage_configs
   SET external_id = encode(
           sha256(
               convert_to('silicon-briefcase:byo-storage:v1', 'UTF8')
               || '\x00'::bytea
               || convert_to(org_id, 'UTF8')
           ),
           'hex'
       )
 WHERE external_id IS NULL;
ALTER TABLE briefcase.organization_storage_configs
    ENABLE TRIGGER organization_storage_configs_set_updated_at;
ALTER TABLE briefcase.organization_storage_configs FORCE ROW LEVEL SECURITY;
ALTER TABLE briefcase.organization_storage_configs
    ALTER COLUMN external_id SET NOT NULL,
    ADD CONSTRAINT organization_storage_configs_external_id_shape
        CHECK (external_id ~ '^[0-9a-f]{64}$');

CREATE OR REPLACE FUNCTION briefcase.protect_storage_config_descriptor()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, briefcase
AS $$
BEGIN
    IF (OLD.bucket_name, OLD.region, OLD.role_arn, OLD.bucket_prefix,
        OLD.aws_account_id, OLD.encryption_mode, OLD.kms_key_arn, OLD.external_id)
       IS DISTINCT FROM
       (NEW.bucket_name, NEW.region, NEW.role_arn, NEW.bucket_prefix,
        NEW.aws_account_id, NEW.encryption_mode, NEW.kms_key_arn, NEW.external_id) THEN
        RAISE EXCEPTION USING
            ERRCODE = '23514',
            MESSAGE = 'storage configuration descriptors are immutable';
    END IF;
    RETURN NEW;
END;
$$;

-- Shares to an account are found across drives ("Shared with me").
CREATE INDEX permission_grants_shared_with_idx
    ON briefcase.permission_grants (principal_id, principal_type, org_id)
    WHERE revoked_at IS NULL;

-- Silicons are not open to the world: a share from outside a Silicon's
-- circle (its custodian and the custodian's other Silicons) needs the Silicon
-- or its custodian to have allowed that account first.
CREATE TABLE briefcase.silicon_share_allowances (
    silicon_uuid text COLLATE "C" NOT NULL
        REFERENCES briefcase.accounts (account_uuid) ON DELETE CASCADE,
    allowed_uuid text COLLATE "C" NOT NULL
        REFERENCES briefcase.accounts (account_uuid) ON DELETE CASCADE,
    allowed_by_uuid text COLLATE "C" NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (silicon_uuid, allowed_uuid),
    CHECK (silicon_uuid <> allowed_uuid),
    CHECK (allowed_by_uuid ~ '^[A-Za-z0-9]{3,32}$')
);

CREATE INDEX silicon_share_allowances_allowed_idx
    ON briefcase.silicon_share_allowances (allowed_uuid);

REVOKE ALL ON briefcase.silicon_share_allowances FROM PUBLIC;

-- Narrow cross-drive functions. Every request runs inside one drive under row
-- security; the few questions that span drives are answered only by these
-- SECURITY DEFINER functions, each returning the least it can. Their owner
-- (the role running migrations) reads across drives through the policies
-- below; the API role never matches them.
CREATE POLICY drive_definer_lookup ON briefcase.entries
    FOR SELECT TO CURRENT_USER USING (true);
CREATE POLICY drive_definer_lookup ON briefcase.organizations
    FOR SELECT TO CURRENT_USER USING (true);
CREATE POLICY drive_definer_lookup ON briefcase.permission_grants
    FOR SELECT TO CURRENT_USER USING (true);

-- The drive holding an entry, so a request for it can run inside that drive.
-- Pre-migration organizations are never reported.
CREATE FUNCTION briefcase.entry_drive(p_entry_id uuid)
RETURNS text
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT CASE WHEN count(*) = 1 THEN min(entry.org_id) END
      FROM briefcase.entries AS entry
      JOIN briefcase.organizations AS drive
        ON drive.org_id = entry.org_id
       AND drive.owner_account_uuid = entry.org_id
     WHERE entry.entry_id = p_entry_id
$$;

-- Entries other accounts shared with this one, newest first: one row per
-- entry, with the drive it lives in. Callers still authorize each entry
-- inside its drive before showing anything.
CREATE FUNCTION briefcase.shared_with_principal(
    p_principal_type text,
    p_principal_id text,
    p_before_at timestamptz,
    p_before_entry uuid,
    p_limit integer
)
RETURNS TABLE (drive text, entry_id uuid, shared_at timestamptz)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    WITH shared AS (
        SELECT DISTINCT ON (grantee.org_id, grantee.entry_id)
               grantee.org_id AS drive,
               grantee.entry_id AS entry_id,
               grantee.created_at AS shared_at
          FROM briefcase.permission_grants AS grantee
          JOIN briefcase.organizations AS owner_drive
            ON owner_drive.org_id = grantee.org_id
           AND owner_drive.owner_account_uuid = grantee.org_id
          JOIN briefcase.entries AS shared_entry
            ON shared_entry.org_id = grantee.org_id
           AND shared_entry.entry_id = grantee.entry_id
           AND shared_entry.deleted_at IS NULL
         WHERE grantee.principal_type = p_principal_type
           AND grantee.principal_id = p_principal_id
           AND grantee.org_id <> p_principal_id
           AND grantee.revoked_at IS NULL
           AND (grantee.expires_at IS NULL OR grantee.expires_at > clock_timestamp())
         ORDER BY grantee.org_id, grantee.entry_id, grantee.created_at DESC
    )
    SELECT shared.drive, shared.entry_id, shared.shared_at
      FROM shared
     WHERE p_before_at IS NULL
        OR (shared.shared_at, shared.entry_id) < (p_before_at, p_before_entry)
     ORDER BY shared.shared_at DESC, shared.entry_id DESC
     LIMIT LEAST(GREATEST(p_limit, 1), 101)
$$;

-- The other drives an account holds live shares in, most recently shared
-- first (the scope of a search across drives).
CREATE FUNCTION briefcase.principal_drives(
    p_principal_type text,
    p_principal_id text,
    p_limit integer
)
RETURNS TABLE (drive text, last_shared_at timestamptz)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
    SELECT grantee.org_id, max(grantee.created_at)
      FROM briefcase.permission_grants AS grantee
      JOIN briefcase.organizations AS owner_drive
        ON owner_drive.org_id = grantee.org_id
       AND owner_drive.owner_account_uuid = grantee.org_id
     WHERE grantee.principal_type = p_principal_type
       AND grantee.principal_id = p_principal_id
       AND grantee.org_id <> p_principal_id
       AND grantee.revoked_at IS NULL
       AND (grantee.expires_at IS NULL OR grantee.expires_at > clock_timestamp())
     GROUP BY grantee.org_id
     ORDER BY max(grantee.created_at) DESC, grantee.org_id
     LIMIT LEAST(GREATEST(p_limit, 1), 200)
$$;

-- Writes one notification into the recipient's own drive (its inbox), in the
-- caller's transaction, and creates that drive's bookkeeping on first use.
-- Returns false, writing nothing, when a pre-migration organization holds the
-- recipient's key.
CREATE FUNCTION briefcase.deliver_notification(
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
        OR p_recipient_id IS NULL OR p_recipient_id !~ '^[A-Za-z0-9]{3,32}$'
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

-- account.deleted: ends every live share to the deleted account, in every
-- drive, and records why. Refused unless the account cache already records
-- the deletion.
CREATE FUNCTION briefcase.revoke_deleted_account_shares(
    p_principal_type text,
    p_principal_id text,
    p_request_id text
)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    original text := COALESCE(current_setting('briefcase.org_id', true), '');
    target text;
    affected bigint;
    revoked bigint := 0;
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM briefcase.accounts
         WHERE account_uuid = p_principal_id AND status = 'deleted'
    ) THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'shares are ended this way only for a deleted account';
    END IF;
    FOR target IN
        SELECT DISTINCT grantee.org_id
          FROM briefcase.permission_grants AS grantee
         WHERE grantee.principal_type = p_principal_type
           AND grantee.principal_id = p_principal_id
           AND grantee.revoked_at IS NULL
         ORDER BY grantee.org_id
    LOOP
        PERFORM set_config('briefcase.org_id', target, true);
        INSERT INTO briefcase.organization_members
            (org_id, actor_type, actor_id, org_role, membership_status, iam_version)
        VALUES (target, p_principal_type, p_principal_id, 'member', 'active', 0)
        ON CONFLICT (org_id, actor_type, actor_id) DO NOTHING;
        WITH ended AS (
            UPDATE briefcase.permission_grants AS grantee
               SET revoked_at = clock_timestamp(),
                   revoked_by_type = p_principal_type,
                   revoked_by_id = p_principal_id
             WHERE grantee.org_id = target
               AND grantee.principal_type = p_principal_type
               AND grantee.principal_id = p_principal_id
               AND grantee.revoked_at IS NULL
         RETURNING grantee.entry_id, grantee.grant_id
        )
        INSERT INTO briefcase.audit_events
            (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata)
        SELECT target, briefcase.new_uuid_v7(), ended.entry_id, p_principal_type, p_principal_id,
               'permission.revoked_account_deleted.v1', p_request_id,
               jsonb_build_object('grant_id', ended.grant_id)
          FROM ended;
        GET DIAGNOSTICS affected = ROW_COUNT;
        revoked := revoked + affected;
    END LOOP;
    PERFORM set_config('briefcase.org_id', original, true);
    RETURN revoked;
END;
$$;

-- account.deleted: everything in the deleted account's own drive is deleted
-- permanently (one deletion batch, purged by the worker at once, objects in a
-- bring-your-own bucket included), its links and shares end and its inbox is
-- cleared. Content it created in other accounts' drives belongs to those
-- drives and is untouched. Refused unless the account cache records the
-- deletion.
CREATE FUNCTION briefcase.erase_deleted_account_drive(p_account_uuid text, p_request_id text)
RETURNS bigint
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, briefcase
AS $$
DECLARE
    original text := COALESCE(current_setting('briefcase.org_id', true), '');
    erased_at timestamptz := clock_timestamp();
    batch uuid := briefcase.new_uuid_v7();
    owner_type text;
    erased bigint := 0;
BEGIN
    SELECT account.kind INTO owner_type
      FROM briefcase.accounts AS account
     WHERE account.account_uuid = p_account_uuid AND account.status = 'deleted';
    IF owner_type IS NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '42501',
            MESSAGE = 'a drive is erased this way only for a deleted account';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM briefcase.organizations
         WHERE org_id = p_account_uuid AND owner_account_uuid = p_account_uuid
    ) THEN
        RETURN 0;
    END IF;
    PERFORM set_config('briefcase.org_id', p_account_uuid, true);
    INSERT INTO briefcase.organization_members
        (org_id, actor_type, actor_id, org_role, membership_status, iam_version)
    VALUES (p_account_uuid, owner_type, p_account_uuid, 'owner', 'active', 0)
    ON CONFLICT (org_id, actor_type, actor_id) DO NOTHING;
    UPDATE briefcase.entries
       SET deletion_batch_id = batch, deleted_at = erased_at, purge_after = erased_at,
           link_public = false, link_expires_at = NULL
     WHERE org_id = p_account_uuid AND deleted_at IS NULL;
    GET DIAGNOSTICS erased = ROW_COUNT;
    UPDATE briefcase.entries
       SET purge_after = LEAST(purge_after, erased_at),
           link_public = false, link_expires_at = NULL
     WHERE org_id = p_account_uuid AND deleted_at IS NOT NULL;
    UPDATE briefcase.permission_grants
       SET revoked_at = erased_at, revoked_by_type = owner_type, revoked_by_id = p_account_uuid
     WHERE org_id = p_account_uuid AND revoked_at IS NULL;
    DELETE FROM briefcase.notifications WHERE org_id = p_account_uuid;
    INSERT INTO briefcase.audit_events
        (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata)
    VALUES (p_account_uuid, briefcase.new_uuid_v7(), NULL, owner_type, p_account_uuid,
            'drive.erased_account_deleted.v1', p_request_id,
            jsonb_build_object('entries', erased, 'deletion_batch_id', batch));
    PERFORM set_config('briefcase.org_id', original, true);
    RETURN erased;
END;
$$;

REVOKE ALL ON FUNCTION briefcase.entry_drive(uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.shared_with_principal(text, text, timestamptz, uuid, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.principal_drives(text, text, integer) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.deliver_notification(text, text, text, text, text, uuid, jsonb) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.revoke_deleted_account_shares(text, text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.erase_deleted_account_drive(text, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION briefcase.reserve_drive_root_names() FROM PUBLIC;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_api') THEN
        GRANT SELECT, INSERT, DELETE ON briefcase.silicon_share_allowances TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.entry_drive(uuid) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.shared_with_principal(text, text, timestamptz, uuid, integer) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.principal_drives(text, text, integer) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.deliver_notification(text, text, text, text, text, uuid, jsonb) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.revoke_deleted_account_shares(text, text, text) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.erase_deleted_account_drive(text, text) TO briefcase_api;
        GRANT EXECUTE ON FUNCTION briefcase.reserve_drive_root_names() TO briefcase_api;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = 'briefcase_worker') THEN
        GRANT SELECT, DELETE ON briefcase.silicon_share_allowances TO briefcase_worker;
        GRANT EXECUTE ON FUNCTION briefcase.reserve_drive_root_names() TO briefcase_worker;
    END IF;
END;
$$;
