-- Everything link-identities may change, named by paths instead of the ids
-- a run generates, so two databases (or one before and after) compare.
SELECT jsonb_build_object(
    'entries', (
        SELECT jsonb_agg(jsonb_build_array(
                   entry.org_id, entry.path, entry.entry_type, entry.system_kind, entry.owner_id,
                   entry.created_by_id, entry.updated_by_id, entry.root_type, entry.tag_id,
                   entry.link_public, entry.origin_app_id, entry.deleted_at IS NOT NULL, entry.name)
               ORDER BY entry.org_id, entry.path COLLATE "C", entry.entry_id)
          FROM briefcase.entries AS entry),
    'versions', (
        SELECT jsonb_agg(jsonb_build_array(version.org_id, entry.path, version.created_by_id,
                                           version.storage_config_id, version.size_bytes)
               ORDER BY version.org_id, entry.path COLLATE "C")
          FROM briefcase.entry_versions AS version
          JOIN briefcase.entries AS entry
            ON entry.org_id = version.org_id AND entry.entry_id = version.entry_id),
    'grants', (
        SELECT jsonb_agg(jsonb_build_array(share.org_id, entry.path, share.principal_type,
                                           share.principal_id, share.access_mask,
                                           share.inherits_to_descendants, share.granted_by_id,
                                           share.revoked_at IS NULL, share.expires_at)
               ORDER BY share.org_id, entry.path COLLATE "C", share.principal_id, share.access_mask)
          FROM briefcase.permission_grants AS share
          JOIN briefcase.entries AS entry
            ON entry.org_id = share.org_id AND entry.entry_id = share.entry_id),
    'tag_grants', (
        SELECT jsonb_agg(jsonb_build_array(share.org_id, entry.path, share.tag_id,
                                           share.revoked_at IS NULL, share.revoked_by_id)
               ORDER BY share.org_id, entry.path COLLATE "C")
          FROM briefcase.tag_permission_grants AS share
          JOIN briefcase.entries AS entry
            ON entry.org_id = share.org_id AND entry.entry_id = share.entry_id),
    'tags', (
        SELECT jsonb_agg(jsonb_build_array(org_id, tag_id, name, lifecycle_status)
               ORDER BY org_id, tag_id)
          FROM briefcase.organization_tags),
    'members', (
        SELECT jsonb_agg(jsonb_build_array(org_id, actor_type, actor_id, org_role, membership_status)
               ORDER BY org_id, actor_type, actor_id)
          FROM briefcase.organization_members),
    'member_tags', (
        SELECT jsonb_agg(jsonb_build_array(org_id, actor_id, tag_id) ORDER BY org_id, actor_id, tag_id)
          FROM briefcase.organization_member_tags),
    'drives', (
        SELECT jsonb_agg(jsonb_build_array(org_id, owner_account_uuid) ORDER BY org_id)
          FROM briefcase.organizations),
    'notifications', (
        SELECT jsonb_agg(jsonb_build_array(org_id, recipient_id, actor_id, entry_id, details)
               ORDER BY org_id, notification_id)
          FROM briefcase.notifications),
    'usage', (
        SELECT jsonb_agg(jsonb_build_array(org_id, stored_bytes, daily_upload_limit_bytes,
                                           storage_limit_bytes)
               ORDER BY org_id)
          FROM briefcase.organization_usage),
    'configs', (
        SELECT jsonb_agg(jsonb_build_array(org_id, storage_config_id, status, external_id,
                                           created_by_id)
               ORDER BY org_id, storage_config_id)
          FROM briefcase.organization_storage_configs),
    'multipart', (
        SELECT jsonb_agg(jsonb_build_array(upload.org_id, parent.path, completed.path, upload.owner_id)
               ORDER BY upload.org_id, upload.upload_id)
          FROM briefcase.multipart_uploads AS upload
          LEFT JOIN briefcase.entries AS parent
            ON parent.org_id = upload.org_id AND parent.entry_id = upload.parent_entry_id
          LEFT JOIN briefcase.entries AS completed
            ON completed.org_id = upload.org_id AND completed.entry_id = upload.completed_entry_id),
    'audit', (
        SELECT jsonb_agg(jsonb_build_array(org_id, entry_id, actor_id, action)
               ORDER BY org_id, entry_id, action, actor_id)
          FROM briefcase.audit_events),
    'search', (
        SELECT jsonb_agg(jsonb_build_array(document.org_id, entry.path) ORDER BY document.org_id, entry.path COLLATE "C")
          FROM briefcase.search_documents AS document
          JOIN briefcase.entries AS entry
            ON entry.org_id = document.org_id AND entry.entry_id = document.entry_id),
    'identifier_aliases', (
        SELECT jsonb_agg(jsonb_build_array(alias.org_id, alias.legacy_path, entry.path)
               ORDER BY alias.org_id, alias.legacy_path)
          FROM briefcase.identifier_path_aliases AS alias
          JOIN briefcase.entries AS entry
            ON entry.org_id = alias.org_id AND entry.entry_id = alias.entry_id),
    'aliases', (
        SELECT jsonb_agg(jsonb_build_array(alias.legacy_org, alias.legacy_path, alias.drive, entry.path)
               ORDER BY alias.legacy_org, alias.legacy_path COLLATE "C")
          FROM briefcase.legacy_path_aliases AS alias
          JOIN briefcase.entries AS entry
            ON entry.org_id = alias.drive AND entry.entry_id = alias.entry_id),
    'accounts', (
        SELECT jsonb_agg(jsonb_build_array(account_uuid, kind, public_id) ORDER BY account_uuid)
          FROM briefcase.accounts),
    'closure', (
        SELECT jsonb_agg(jsonb_build_array(link.org_id, above.path, below.path, link.depth)
               ORDER BY link.org_id, above.path COLLATE "C", below.path COLLATE "C")
          FROM briefcase.entry_closure AS link
          JOIN briefcase.entries AS above
            ON above.org_id = link.org_id AND above.entry_id = link.ancestor_id
          JOIN briefcase.entries AS below
            ON below.org_id = link.org_id AND below.entry_id = link.descendant_id)
)
