-- One organization from before personal drives, as origin/main's schema
-- (migrations 0001–0047) stores it: org `tos`, owned by c:saket, with Bob,
-- the Silicon Scout, c:gone (no Silicon Accounts account) and c:ex (removed).
-- Public, private, tag and app spaces, versions (one in the organization's
-- own bucket), a share, a tag share, a notification, a legacy path alias, a
-- finished multipart upload and a limit override.

INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status)
VALUES ('tos', 7, 'active');

INSERT INTO briefcase.organization_members
    (org_id, actor_type, actor_id, org_role, membership_status, iam_version, principal_id)
VALUES
    ('tos', 'carbon', 'c:saket', 'owner', 'active', 1, '01900000-0000-7000-8000-000000000001'),
    ('tos', 'carbon', 'c:bob', 'member', 'active', 1, '01900000-0000-7000-8000-000000000002'),
    ('tos', 'silicon', 'si:scout', 'member', 'active', 1, NULL),
    ('tos', 'carbon', 'c:gone', 'member', 'active', 1, NULL),
    ('tos', 'carbon', 'c:ex', 'member', 'removed', 2, NULL);

INSERT INTO briefcase.iam_identity_bindings (environment_id, identity_kind, public_id, local_id)
VALUES ('00000000-0000-0000-0000-000000000000', 'carbon', 'c:saket',
        '01900000-0000-7000-8000-000000000001')
ON CONFLICT DO NOTHING;

INSERT INTO briefcase.organization_tags (org_id, tag_id, name, lifecycle_status, iam_version)
VALUES ('tos', 'tag-design', 'design', 'active', 1);
INSERT INTO briefcase.organization_member_tags (org_id, actor_type, actor_id, tag_id, iam_version)
VALUES ('tos', 'carbon', 'c:bob', 'tag-design', 1);

INSERT INTO briefcase.organization_storage_configs (
    org_id, storage_config_id, status, bucket_name, region, role_arn, bucket_prefix,
    aws_account_id, encryption_mode, validated_at, created_by_type, created_by_id
) VALUES (
    'tos', '01900000-0000-7000-8000-000000000601', 'active', 'tos-own-bucket', 'us-east-1',
    'arn:aws:iam::123456789012:role/briefcase-access', 'briefcase/', '123456789012', 'sse_s3',
    clock_timestamp(), 'carbon', 'c:saket'
);

-- Structure, exactly as Briefcase reconciled it.
INSERT INTO briefcase.entries
    (org_id, entry_id, parent_id, entry_type, name, root_type, tag_id, system_kind,
     owner_type, owner_id, origin_app_id, created_by_type, created_by_id, updated_by_type, updated_by_id)
VALUES
    ('tos', '01900000-0000-7000-8000-000000000101', NULL, 'folder', 'public', 'public', NULL, 'public_root',
     'carbon', 'c:saket', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000102', NULL, 'folder', 'private', 'private', NULL, 'private_root',
     'carbon', 'c:saket', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000103', '01900000-0000-7000-8000-000000000102', 'folder',
     'c:saket', 'private', NULL, 'actor_root', 'carbon', 'c:saket', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000104', '01900000-0000-7000-8000-000000000102', 'folder',
     'c:bob', 'private', NULL, 'actor_root', 'carbon', 'c:bob', NULL, 'carbon', 'c:bob', 'carbon', 'c:bob'),
    ('tos', '01900000-0000-7000-8000-000000000105', '01900000-0000-7000-8000-000000000102', 'folder',
     'si:scout', 'private', NULL, 'actor_root', 'silicon', 'si:scout', NULL, 'silicon', 'si:scout', 'silicon', 'si:scout'),
    ('tos', '01900000-0000-7000-8000-000000000106', '01900000-0000-7000-8000-000000000102', 'folder',
     'c:gone', 'private', NULL, 'actor_root', 'carbon', 'c:gone', NULL, 'carbon', 'c:gone', 'carbon', 'c:gone'),
    ('tos', '01900000-0000-7000-8000-000000000107', NULL, 'folder', 'design', 'tag', 'tag-design', 'tag_root',
     'carbon', 'c:saket', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000108', NULL, 'folder', 'apps', 'public', NULL, 'apps_root',
     'carbon', 'c:saket', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000109', '01900000-0000-7000-8000-000000000108', 'folder',
     'waveform', 'public', NULL, 'app_root', 'carbon', 'c:saket', 'waveform', 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000110', '01900000-0000-7000-8000-000000000109', 'folder',
     'public', 'public', NULL, 'app_public', 'carbon', 'c:saket', 'waveform', 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000111', '01900000-0000-7000-8000-000000000109', 'folder',
     'private', 'private', NULL, 'app_private', 'carbon', 'c:saket', 'waveform', 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000112', '01900000-0000-7000-8000-000000000111', 'folder',
     'c:bob', 'private', NULL, 'app_actor', 'carbon', 'c:bob', 'waveform', 'carbon', 'c:bob', 'carbon', 'c:bob');

-- Folders people made.
INSERT INTO briefcase.entries
    (org_id, entry_id, parent_id, entry_type, name, root_type, tag_id,
     owner_type, owner_id, created_by_type, created_by_id, updated_by_type, updated_by_id)
VALUES
    ('tos', '01900000-0000-7000-8000-000000000201', '01900000-0000-7000-8000-000000000101', 'folder',
     'handbook', 'public', NULL, 'carbon', 'c:bob', 'carbon', 'c:bob', 'carbon', 'c:bob'),
    ('tos', '01900000-0000-7000-8000-000000000203', '01900000-0000-7000-8000-000000000103', 'folder',
     'notes', 'private', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000204', '01900000-0000-7000-8000-000000000103', 'folder',
     'apps', 'private', NULL, 'carbon', 'c:saket', 'carbon', 'c:saket', 'carbon', 'c:saket'),
    ('tos', '01900000-0000-7000-8000-000000000209', '01900000-0000-7000-8000-000000000107', 'folder',
     'mockups', 'tag', 'tag-design', 'carbon', 'c:saket', 'carbon', 'c:saket', 'carbon', 'c:saket');

-- Files, each with its current version.
INSERT INTO briefcase.entries
    (org_id, entry_id, parent_id, entry_type, name, root_type, tag_id, owner_type, owner_id,
     origin_app_id, content_type, size_bytes, current_version_id,
     created_by_type, created_by_id, updated_by_type, updated_by_id, link_public)
VALUES
    ('tos', '01900000-0000-7000-8000-000000000202', '01900000-0000-7000-8000-000000000201', 'file',
     'guide.md', 'public', NULL, 'carbon', 'c:bob', NULL, 'text/markdown', 100,
     '01900000-0000-7000-8000-000000000302', 'carbon', 'c:bob', 'carbon', 'c:bob', true),
    ('tos', '01900000-0000-7000-8000-000000000205', '01900000-0000-7000-8000-000000000103', 'file',
     'big.bin', 'private', NULL, 'carbon', 'c:saket', NULL, 'application/octet-stream', 1000,
     '01900000-0000-7000-8000-000000000305', 'carbon', 'c:saket', 'carbon', 'c:saket', false),
    ('tos', '01900000-0000-7000-8000-000000000206', '01900000-0000-7000-8000-000000000104', 'file',
     'todo.txt', 'private', NULL, 'carbon', 'c:bob', NULL, 'text/plain', 20,
     '01900000-0000-7000-8000-000000000306', 'carbon', 'c:bob', 'carbon', 'c:bob', false),
    ('tos', '01900000-0000-7000-8000-000000000207', '01900000-0000-7000-8000-000000000105', 'file',
     'run.log', 'private', NULL, 'silicon', 'si:scout', NULL, 'text/plain', 30,
     '01900000-0000-7000-8000-000000000307', 'silicon', 'si:scout', 'silicon', 'si:scout', false),
    ('tos', '01900000-0000-7000-8000-000000000208', '01900000-0000-7000-8000-000000000106', 'file',
     'old.txt', 'private', NULL, 'carbon', 'c:gone', NULL, 'text/plain', 40,
     '01900000-0000-7000-8000-000000000308', 'carbon', 'c:gone', 'carbon', 'c:gone', false),
    ('tos', '01900000-0000-7000-8000-000000000210', '01900000-0000-7000-8000-000000000110', 'file',
     'take.wav', 'public', NULL, 'carbon', 'c:saket', 'waveform', 'audio/wav', 500,
     '01900000-0000-7000-8000-000000000310', 'carbon', 'c:saket', 'carbon', 'c:saket', false),
    ('tos', '01900000-0000-7000-8000-000000000211', '01900000-0000-7000-8000-000000000112', 'file',
     'bob-take.wav', 'private', NULL, 'carbon', 'c:bob', 'waveform', 'audio/wav', 600,
     '01900000-0000-7000-8000-000000000311', 'carbon', 'c:bob', 'carbon', 'c:bob', false);

INSERT INTO briefcase.entry_versions (
    org_id, entry_id, version_id, version_number, source, storage_backend, storage_config_id,
    bucket_name, storage_region, storage_prefix, storage_encryption_mode, object_key,
    checksum_algorithm, checksum_type, checksum_value, size_bytes, content_type,
    created_by_type, created_by_id, content_sha256
)
SELECT 'tos', file.entry_id, file.version_id, 1, 'upload',
       CASE WHEN file.own_bucket THEN 'organization' ELSE 'platform' END,
       CASE WHEN file.own_bucket THEN '01900000-0000-7000-8000-000000000601'::uuid END,
       CASE WHEN file.own_bucket THEN 'tos-own-bucket' ELSE 'briefcase-platform' END,
       'us-east-1', 'organizations/tos', 'sse_s3', 'objects/' || file.version_id,
       'sha256', 'full_object', 'checksum', file.size, file.content_type,
       file.owner_type, file.owner_id, repeat('a', 64)
  FROM (VALUES
      ('01900000-0000-7000-8000-000000000202'::uuid, '01900000-0000-7000-8000-000000000302'::uuid, 100, 'text/markdown', 'carbon', 'c:bob', false),
      ('01900000-0000-7000-8000-000000000205'::uuid, '01900000-0000-7000-8000-000000000305'::uuid, 1000, 'application/octet-stream', 'carbon', 'c:saket', false),
      ('01900000-0000-7000-8000-000000000206'::uuid, '01900000-0000-7000-8000-000000000306'::uuid, 20, 'text/plain', 'carbon', 'c:bob', true),
      ('01900000-0000-7000-8000-000000000207'::uuid, '01900000-0000-7000-8000-000000000307'::uuid, 30, 'text/plain', 'silicon', 'si:scout', false),
      ('01900000-0000-7000-8000-000000000208'::uuid, '01900000-0000-7000-8000-000000000308'::uuid, 40, 'text/plain', 'carbon', 'c:gone', false),
      ('01900000-0000-7000-8000-000000000210'::uuid, '01900000-0000-7000-8000-000000000310'::uuid, 500, 'audio/wav', 'carbon', 'c:saket', false),
      ('01900000-0000-7000-8000-000000000211'::uuid, '01900000-0000-7000-8000-000000000311'::uuid, 600, 'audio/wav', 'carbon', 'c:bob', false)
  ) AS file (entry_id, version_id, size, content_type, owner_type, owner_id, own_bucket);

INSERT INTO briefcase.search_documents (org_id, entry_id, filename)
VALUES ('tos', '01900000-0000-7000-8000-000000000202', 'guide.md');

-- Saket shared his notes with Bob; the design tag may add to them.
INSERT INTO briefcase.permission_grants (
    org_id, entry_id, grant_id, principal_type, principal_id, inherits_to_descendants,
    granted_by_type, granted_by_id, access_mask
) VALUES (
    'tos', '01900000-0000-7000-8000-000000000203', '01900000-0000-7000-8000-000000000401',
    'carbon', 'c:bob', true, 'carbon', 'c:saket', 1
);
INSERT INTO briefcase.tag_permission_grants (
    org_id, entry_id, grant_id, tag_id, access_mask, inherits_to_descendants,
    granted_by_type, granted_by_id
) VALUES (
    'tos', '01900000-0000-7000-8000-000000000203', '01900000-0000-7000-8000-000000000402',
    'tag-design', 3, true, 'carbon', 'c:saket'
);

INSERT INTO briefcase.notifications (
    org_id, notification_id, recipient_type, recipient_id, kind, actor_type, actor_id, entry_id, details
) VALUES (
    'tos', '01900000-0000-7000-8000-000000000501', 'carbon', 'c:bob', 'access_granted',
    'carbon', 'c:saket', '01900000-0000-7000-8000-000000000203',
    '{"name": "notes", "path": "private/c:saket/notes", "entry_type": "folder", "access_mask": 1}'
);
-- The invitation email went out long ago.
UPDATE briefcase.outbox_events SET status = 'delivered', delivered_at = clock_timestamp()
 WHERE org_id = 'tos';

INSERT INTO briefcase.audit_events (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id)
VALUES ('tos', '01900000-0000-7000-8000-000000000801', '01900000-0000-7000-8000-000000000202',
        'carbon', 'c:bob', 'entry.created.v1', 'fixture');

-- Notes were addressed as private/me:tos/notes before the 2.0.0 id change.
INSERT INTO briefcase.identifier_path_aliases (org_id, legacy_path, entry_id)
VALUES ('tos', 'private/me:tos/notes', '01900000-0000-7000-8000-000000000203');

INSERT INTO briefcase.multipart_uploads (
    org_id, upload_id, parent_entry_id, owner_type, owner_id, name, content_type,
    declared_size_bytes, part_size_bytes, expected_part_count, storage_backend, bucket_name,
    storage_region, storage_prefix, storage_encryption_mode, object_key, provider_upload_id,
    status, completed_entry_id, expires_at, completed_at
) VALUES (
    'tos', '01900000-0000-7000-8000-000000000701', '01900000-0000-7000-8000-000000000103',
    'carbon', 'c:saket', 'big.bin', 'application/octet-stream', 104857601, 8388608, 13,
    'platform', 'briefcase-platform', 'us-east-1', 'organizations/tos', 'sse_s3',
    'objects/big', 'provider-upload-1', 'completed', '01900000-0000-7000-8000-000000000205',
    clock_timestamp() + interval '1 day', clock_timestamp()
);

UPDATE briefcase.organization_usage SET storage_limit_bytes = 5000000 WHERE org_id = 'tos';
