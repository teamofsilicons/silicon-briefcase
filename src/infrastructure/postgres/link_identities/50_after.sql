-- briefcase-migrate link-identities, part 5: everything that follows the
-- moves: participants, inboxes, bucket settings, tags, snapshot shares,
-- usage, aliases, the account cache and the identity links.

-- Every account a row in a touched drive names becomes a participant of it
-- (rows name their actors through participants).
CREATE OR REPLACE FUNCTION pg_temp.link_participants(p_drive text)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    added record;
BEGIN
    FOR added IN
        INSERT INTO briefcase.organization_members
            (org_id, actor_type, actor_id, org_role, membership_status, iam_version)
        SELECT DISTINCT p_drive, ref.kind, ref.id,
               CASE WHEN ref.id = p_drive THEN 'owner' ELSE 'member' END, 'active', 0
          FROM (
              SELECT owner_type, owner_id FROM briefcase.entries WHERE org_id = p_drive
              UNION SELECT created_by_type, created_by_id FROM briefcase.entries WHERE org_id = p_drive
              UNION SELECT updated_by_type, updated_by_id FROM briefcase.entries WHERE org_id = p_drive
              UNION SELECT created_by_type, created_by_id FROM briefcase.entry_versions WHERE org_id = p_drive
              UNION SELECT principal_type, principal_id FROM briefcase.permission_grants WHERE org_id = p_drive
              UNION SELECT granted_by_type, granted_by_id FROM briefcase.permission_grants WHERE org_id = p_drive
              UNION SELECT revoked_by_type, revoked_by_id FROM briefcase.permission_grants WHERE org_id = p_drive
              UNION SELECT requested_by_type, requested_by_id FROM briefcase.access_requests WHERE org_id = p_drive
              UNION SELECT decided_by_type, decided_by_id FROM briefcase.access_requests WHERE org_id = p_drive
              UNION SELECT actor_type, actor_id FROM briefcase.audit_events WHERE org_id = p_drive
              UNION SELECT owner_type, owner_id FROM briefcase.multipart_uploads WHERE org_id = p_drive
              UNION SELECT recipient_type, recipient_id FROM briefcase.notifications WHERE org_id = p_drive
              UNION SELECT created_by_type, created_by_id FROM briefcase.organization_storage_configs WHERE org_id = p_drive
          ) AS ref (kind, id)
         WHERE ref.kind IS NOT NULL AND ref.id IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM briefcase.organization_members AS member
                WHERE member.org_id = p_drive AND member.actor_type = ref.kind
                  AND member.actor_id = ref.id)
        RETURNING actor_type, actor_id
    LOOP
        PERFORM pg_temp.link_record('participant', p_drive,
            jsonb_build_object('actor_type', added.actor_type, 'actor_id', added.actor_id), NULL);
    END LOOP;
END;
$$;

-- Notifications move to their recipient's own drive (its inbox now), and
-- their subject snapshot follows the entry when it moved.
CREATE OR REPLACE FUNCTION pg_temp.link_move_notifications()
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
    subject record;
    new_details jsonb;
    moved integer := 0;
BEGIN
    FOR item IN
        SELECT notification.* FROM briefcase.notifications AS notification
          JOIN link_legacy_orgs ON link_legacy_orgs.org = notification.org_id
         WHERE pg_temp.link_is_mapped(notification.recipient_id)
         ORDER BY notification.org_id, notification.notification_id
    LOOP
        PERFORM pg_temp.link_drive(item.recipient_id);
        new_details := item.details;
        SELECT moved_entry.drive, entry.path INTO subject
          FROM link_moved AS moved_entry
          JOIN briefcase.entries AS entry
            ON entry.org_id = moved_entry.drive AND entry.entry_id = moved_entry.entry_id
         WHERE moved_entry.entry_id = item.entry_id;
        IF subject.drive IS NOT NULL AND subject.path <> '.' THEN
            new_details := new_details || jsonb_build_object('drive', subject.drive, 'path', subject.path);
        END IF;
        PERFORM pg_temp.link_record('notification', item.recipient_id,
            jsonb_build_object('notification_id', item.notification_id),
            jsonb_build_object('org_id', item.org_id, 'details', item.details));
        UPDATE briefcase.notifications
           SET org_id = item.recipient_id, details = new_details
         WHERE org_id = item.org_id AND notification_id = item.notification_id;
        moved := moved + 1;
    END LOOP;
    RETURN moved;
END;
$$;

-- Bring-your-own bucket settings: a drive that now holds versions or uploads
-- stored with an organization's settings gets a copy of them (same External
-- ID, so the bucket's trust policy keeps working). The owner's drive takes
-- over the organization's active settings unless it has its own; every other
-- copy only reads what is already stored.
CREATE OR REPLACE FUNCTION pg_temp.link_storage_configs()
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    item record;
BEGIN
    FOR item IN
        SELECT DISTINCT needed.drive, config.*
          FROM (
              SELECT version.org_id AS drive, version.storage_config_id
                FROM briefcase.entry_versions AS version
               WHERE version.storage_config_id IS NOT NULL
                 AND version.org_id IN (SELECT DISTINCT drive FROM link_moved)
              UNION
              SELECT upload.org_id, upload.storage_config_id
                FROM briefcase.multipart_uploads AS upload
               WHERE upload.storage_config_id IS NOT NULL
                 AND upload.org_id IN (SELECT DISTINCT drive FROM link_moved)
              UNION
              SELECT owners.owner_uuid, config.storage_config_id
                FROM link_orgs AS owners
                JOIN briefcase.organization_storage_configs AS config
                  ON config.org_id = owners.org AND config.status = 'active'
          ) AS needed
          JOIN briefcase.organization_storage_configs AS config
            ON config.storage_config_id = needed.storage_config_id
           AND config.org_id IN (SELECT org FROM link_legacy_orgs)
         WHERE NOT EXISTS (
             SELECT 1 FROM briefcase.organization_storage_configs AS present
              WHERE present.org_id = needed.drive
                AND present.storage_config_id = needed.storage_config_id)
    LOOP
        INSERT INTO briefcase.organization_storage_configs (
            org_id, storage_config_id, status, bucket_name, region, role_arn, bucket_prefix,
            aws_account_id, encryption_mode, kms_key_arn, validated_at, validation_failure_code,
            validation_failure_reason, created_by_type, created_by_id, created_at, updated_at,
            external_id
        ) VALUES (
            item.drive, item.storage_config_id,
            CASE
                WHEN item.status = 'active'
                     AND EXISTS (SELECT 1 FROM link_orgs WHERE org = item.org_id AND owner_uuid = item.drive)
                     AND NOT EXISTS (SELECT 1 FROM briefcase.organization_storage_configs AS own
                                      WHERE own.org_id = item.drive AND own.status = 'active')
                    THEN 'active'
                WHEN item.status = 'active' THEN 'superseded'
                ELSE item.status
            END,
            item.bucket_name, item.region, item.role_arn, item.bucket_prefix, item.aws_account_id,
            item.encryption_mode, item.kms_key_arn, item.validated_at, item.validation_failure_code,
            item.validation_failure_reason, item.created_by_type, item.created_by_id,
            item.created_at, item.updated_at, item.external_id
        );
        PERFORM pg_temp.link_record('storage_config', item.drive,
            jsonb_build_object('storage_config_id', item.storage_config_id), NULL);
    END LOOP;
END;
$$;

-- Tag shares become shares to each account that had the tag at the
-- cutover (same rights, inheritance and expiry). The tag share itself is
-- kept, revoked, with a copy of its tag so it still names one.
CREATE OR REPLACE FUNCTION pg_temp.link_expand_tag_grants()
RETURNS integer
LANGUAGE plpgsql
AS $$
DECLARE
    tag_grant record;
    member record;
    expanded integer := 0;
    legacy text;
BEGIN
    FOR tag_grant IN
        SELECT grant_row.*, moved_entry.legacy_org
          FROM briefcase.tag_permission_grants AS grant_row
          JOIN link_moved AS moved_entry ON moved_entry.entry_id = grant_row.entry_id
         WHERE grant_row.org_id = moved_entry.drive
         ORDER BY grant_row.org_id, grant_row.grant_id
    LOOP
        legacy := tag_grant.legacy_org;
        IF NOT EXISTS (SELECT 1 FROM briefcase.organization_tags
                        WHERE org_id = tag_grant.org_id AND tag_id = tag_grant.tag_id) THEN
            INSERT INTO briefcase.organization_tags (org_id, tag_id, name, lifecycle_status, iam_version)
            SELECT tag_grant.org_id, tag.tag_id, tag.tag_id, 'removed', tag.iam_version
              FROM briefcase.organization_tags AS tag
             WHERE tag.org_id = legacy AND tag.tag_id = tag_grant.tag_id;
            PERFORM pg_temp.link_record('tag', tag_grant.org_id,
                jsonb_build_object('tag_id', tag_grant.tag_id), NULL);
        END IF;
        IF tag_grant.revoked_at IS NOT NULL
            OR (tag_grant.expires_at IS NOT NULL AND tag_grant.expires_at <= clock_timestamp()) THEN
            CONTINUE;
        END IF;
        FOR member IN
            SELECT member_tag.actor_type, member_tag.actor_id
              FROM briefcase.organization_member_tags AS member_tag
              JOIN briefcase.organization_members AS member_row
                ON member_row.org_id = member_tag.org_id
               AND member_row.actor_type = member_tag.actor_type
               AND member_row.actor_id = member_tag.actor_id
               AND member_row.membership_status = 'active'
             WHERE member_tag.org_id = legacy AND member_tag.tag_id = tag_grant.tag_id
               AND pg_temp.link_is_mapped(member_tag.actor_id)
        LOOP
            PERFORM pg_temp.link_grant(tag_grant.org_id, tag_grant.entry_id, member.actor_type,
                member.actor_id, tag_grant.access_mask, tag_grant.inherits_to_descendants,
                tag_grant.granted_by_type, tag_grant.granted_by_id, tag_grant.expires_at);
            expanded := expanded + 1;
        END LOOP;
        PERFORM pg_temp.link_record('tag_grant', tag_grant.org_id,
            jsonb_build_object('grant_id', tag_grant.grant_id),
            jsonb_build_object('revoked_at', tag_grant.revoked_at,
                               'revoked_by_type', tag_grant.revoked_by_type,
                               'revoked_by_id', tag_grant.revoked_by_id));
        UPDATE briefcase.tag_permission_grants
           SET revoked_at = clock_timestamp(),
               revoked_by_type = pg_temp.link_kind_of(tag_grant.org_id),
               revoked_by_id = tag_grant.org_id
         WHERE org_id = tag_grant.org_id AND grant_id = tag_grant.grant_id;
    END LOOP;
    RETURN expanded;
END;
$$;

-- One share written directly (shares at the cutover record access members
-- already had, so they bypass the "Silicons are not open to the world" check
-- new shares go through).
CREATE OR REPLACE FUNCTION pg_temp.link_grant(
    p_drive text, p_entry uuid, p_kind text, p_id text, p_mask smallint, p_inherit boolean,
    p_by_kind text, p_by_id text, p_expires timestamptz
)
RETURNS void
LANGUAGE plpgsql
AS $$
DECLARE
    created uuid := briefcase.new_uuid_v7();
    present record;
BEGIN
    IF p_id = p_drive THEN
        RETURN;
    END IF;
    -- One permanent share per account and entry: an existing one gains the
    -- rights (and inheritance) the new one would have given.
    SELECT * INTO present FROM briefcase.permission_grants AS share
     WHERE share.org_id = p_drive AND share.entry_id = p_entry
       AND share.principal_type = p_kind AND share.principal_id = p_id
       AND share.revoked_at IS NULL AND share.expires_at IS NULL;
    IF p_expires IS NULL AND present.grant_id IS NOT NULL THEN
        IF (present.access_mask | p_mask) = present.access_mask
            AND (present.inherits_to_descendants OR NOT p_inherit) THEN
            RETURN;
        END IF;
        PERFORM pg_temp.link_record('grant', p_drive,
            jsonb_build_object('entry_id', p_entry, 'grant_id', present.grant_id),
            jsonb_build_object('access_mask', present.access_mask,
                               'inherits_to_descendants', present.inherits_to_descendants));
        UPDATE briefcase.permission_grants
           SET access_mask = present.access_mask | p_mask,
               inherits_to_descendants = present.inherits_to_descendants OR p_inherit
         WHERE org_id = p_drive AND grant_id = present.grant_id;
        RETURN;
    END IF;
    IF EXISTS (
        SELECT 1 FROM briefcase.permission_grants AS same_share
         WHERE same_share.org_id = p_drive AND same_share.entry_id = p_entry
           AND same_share.principal_type = p_kind AND same_share.principal_id = p_id
           AND same_share.revoked_at IS NULL AND same_share.access_mask = p_mask
           AND same_share.inherits_to_descendants = p_inherit
           AND same_share.expires_at IS NOT DISTINCT FROM p_expires)
    THEN
        RETURN;
    END IF;
    INSERT INTO briefcase.permission_grants (
        org_id, entry_id, grant_id, principal_type, principal_id, inherits_to_descendants,
        granted_by_type, granted_by_id, access_mask, expires_at
    ) VALUES (
        p_drive, p_entry, created, p_kind, p_id, p_inherit, p_by_kind, p_by_id, p_mask, p_expires
    );
    PERFORM pg_temp.link_record('grant', p_drive,
        jsonb_build_object('entry_id', p_entry, 'grant_id', created), NULL);
END;
$$;
