//! Database row representations kept separate from validated domain types.

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

/// One organization projection received from IAM.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct OrganizationRow {
    /// IAM organization identifier.
    pub org_id: String,
    /// Latest applied IAM aggregate version.
    pub iam_version: i64,
    /// Projected lifecycle state.
    pub lifecycle_status: String,
    /// First projection timestamp.
    pub created_at: OffsetDateTime,
    /// Most recent projection timestamp.
    pub updated_at: OffsetDateTime,
}

/// One Carbon or Silicon membership projection.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct OrganizationMemberRow {
    /// IAM organization identifier.
    pub org_id: String,
    /// `carbon` or `silicon`.
    pub actor_type: String,
    /// IAM actor identifier.
    pub actor_id: String,
    /// Immutable IAM principal UUID used by OAuth introspection.
    pub principal_id: Option<Uuid>,
    /// Immutable IAM membership UUID used by OAuth introspection.
    pub membership_id: Option<Uuid>,
    /// IAM epoch invalidated whenever membership authorization changes.
    pub authorization_epoch: Option<i64>,
    /// Current IAM organization role.
    pub org_role: String,
    /// Current membership lifecycle state.
    pub membership_status: String,
    /// Latest applied IAM aggregate version.
    pub iam_version: i64,
    /// First projection timestamp.
    pub created_at: OffsetDateTime,
    /// Most recent projection timestamp.
    pub updated_at: OffsetDateTime,
}

/// One organization tag projection.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct OrganizationTagRow {
    /// IAM organization identifier.
    pub org_id: String,
    /// Immutable IAM tag identifier.
    pub tag_id: String,
    /// Current display and matching name.
    pub name: String,
    /// Current lifecycle state.
    pub lifecycle_status: String,
    /// Latest applied IAM aggregate version.
    pub iam_version: i64,
    /// First projection timestamp.
    pub created_at: OffsetDateTime,
    /// Most recent projection timestamp.
    pub updated_at: OffsetDateTime,
}

/// Persisted file or folder metadata.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EntryRow {
    /// Owning organization.
    pub org_id: String,
    /// `UUIDv7` entry identifier.
    pub entry_id: Uuid,
    /// Parent folder, or `None` for an organization-root entry.
    pub parent_id: Option<Uuid>,
    /// `file` or `folder`.
    pub entry_type: String,
    /// User-visible name.
    pub name: String,
    /// Materialized organization-relative path, maintained by the schema.
    pub path: String,
    /// Inherited `public`, `private`, or `tag` boundary.
    pub root_type: String,
    /// IAM tag identifier for a tag boundary.
    pub tag_id: Option<String>,
    /// Reserved system-entry classification.
    pub system_kind: Option<String>,
    /// Owner actor kind.
    pub owner_type: String,
    /// Owner actor identifier.
    pub owner_id: String,
    /// IAM-verified application that originated the entry.
    pub origin_app_id: Option<String>,
    /// Current media type for files.
    pub content_type: Option<String>,
    /// Current byte size for files.
    pub size_bytes: Option<i64>,
    /// Current immutable content version.
    pub current_version_id: Option<Uuid>,
    /// Actor kind that created the entry.
    pub created_by_type: String,
    /// Actor identifier that created the entry.
    pub created_by_id: String,
    /// Actor kind that last updated the entry.
    pub updated_by_type: String,
    /// Actor identifier that last updated the entry.
    pub updated_by_id: String,
    /// Shared deletion batch for a recoverable subtree.
    pub deletion_batch_id: Option<Uuid>,
    /// Soft-deletion timestamp.
    pub deleted_at: Option<OffsetDateTime>,
    /// Earliest permanent-purge time.
    pub purge_after: Option<OffsetDateTime>,
    /// Creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last metadata update timestamp.
    pub updated_at: OffsetDateTime,
}

/// Immutable metadata for one file content version.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EntryVersionRow {
    /// Owning organization.
    pub org_id: String,
    /// Owning file entry.
    pub entry_id: Uuid,
    /// `UUIDv7` version identifier.
    pub version_id: Uuid,
    /// Monotonic per-entry version number.
    pub version_number: i64,
    /// `upload` or `restore`.
    pub source: String,
    /// Historical source copied by a restore.
    pub restored_from_version_id: Option<Uuid>,
    /// Platform or organization storage selector.
    pub storage_backend: String,
    /// BYO storage configuration snapshot reference.
    pub storage_config_id: Option<Uuid>,
    /// Exact bucket containing the immutable object.
    pub bucket_name: String,
    /// Region snapshotted when the immutable version was published.
    pub storage_region: String,
    /// Tenant prefix snapshotted when the immutable version was published.
    pub storage_prefix: String,
    /// Server-side encryption mode snapshotted for the object.
    pub storage_encryption_mode: String,
    /// KMS key snapshotted for SSE-KMS objects.
    pub storage_kms_key_arn: Option<String>,
    /// Opaque server-generated object key.
    pub object_key: String,
    /// Provider-native object version identifier.
    pub object_version_id: Option<String>,
    /// Provider entity tag.
    pub etag: Option<String>,
    /// Provider checksum algorithm (`sha256` in v1).
    pub checksum_algorithm: String,
    /// Whether the checksum covers a full object or composes multipart digests.
    pub checksum_type: String,
    /// Provider-encoded checksum value.
    pub checksum_value: String,
    /// Object byte size.
    pub size_bytes: i64,
    /// Object media type.
    pub content_type: String,
    /// Creating actor kind.
    pub created_by_type: String,
    /// Creating actor identifier.
    pub created_by_id: String,
    /// Version creation timestamp.
    pub created_at: OffsetDateTime,
}

/// Explicit, independently revocable permission grant.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct PermissionGrantRow {
    /// Owning organization.
    pub org_id: String,
    /// Entry receiving the grant.
    pub entry_id: Uuid,
    /// Grant identifier.
    pub grant_id: Uuid,
    /// Granted principal kind.
    pub principal_type: String,
    /// Granted principal identifier.
    pub principal_id: String,
    /// Bitmask of conveyed read/write/update/delete rights.
    pub access_mask: i16,
    /// Whether the grant flows to descendants.
    pub inherits_to_descendants: bool,
    /// Granting actor kind.
    pub granted_by_type: String,
    /// Granting actor identifier.
    pub granted_by_id: String,
    /// Revocation timestamp.
    pub revoked_at: Option<OffsetDateTime>,
    /// Revoking actor kind.
    pub revoked_by_type: Option<String>,
    /// Revoking actor identifier.
    pub revoked_by_id: Option<String>,
    /// Grant creation timestamp.
    pub created_at: OffsetDateTime,
}

/// Pending or decided request for entry access.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AccessRequestRow {
    /// Owning organization.
    pub org_id: String,
    /// Request identifier.
    pub access_request_id: Uuid,
    /// Requested entry.
    pub entry_id: Uuid,
    /// Requesting actor kind.
    pub requested_by_type: String,
    /// Requesting actor identifier.
    pub requested_by_id: String,
    /// Bitmask of requested rights.
    pub requested_access_mask: i16,
    /// Optional user-supplied reason.
    pub reason: Option<String>,
    /// Pending, approved, or denied state.
    pub status: String,
    /// Bitmask of rights actually granted on approval.
    pub granted_access_mask: Option<i16>,
    /// Decision actor kind.
    pub decided_by_type: Option<String>,
    /// Decision actor identifier.
    pub decided_by_id: Option<String>,
    /// Decision timestamp.
    pub decided_at: Option<OffsetDateTime>,
    /// Grant atomically created by approval.
    pub permission_grant_id: Option<Uuid>,
    /// Request creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Validated organization-owned object storage configuration.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct OrganizationStorageConfigRow {
    /// Owning organization.
    pub org_id: String,
    /// Configuration identifier.
    pub storage_config_id: Uuid,
    /// Validation and activation state.
    pub status: String,
    /// Customer bucket.
    pub bucket_name: String,
    /// AWS region.
    pub region: String,
    /// Assumable IAM role ARN.
    pub role_arn: String,
    /// Tenant-owned key prefix.
    pub bucket_prefix: String,
    /// Expected AWS account identifier.
    pub aws_account_id: String,
    /// SSE-S3 or SSE-KMS database encoding.
    pub encryption_mode: String,
    /// KMS key ARN for SSE-KMS.
    pub kms_key_arn: Option<String>,
    /// Successful validation timestamp.
    pub validated_at: Option<OffsetDateTime>,
    /// Stable redacted validation failure code.
    pub validation_failure_code: Option<String>,
    /// Redacted operator-facing validation failure detail.
    pub validation_failure_reason: Option<String>,
    /// Configuring actor kind.
    pub created_by_type: String,
    /// Configuring actor identifier.
    pub created_by_id: String,
    /// Configuration creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Durable multipart upload session.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct MultipartUploadRow {
    /// Owning organization.
    pub org_id: String,
    /// Briefcase upload identifier.
    pub upload_id: Uuid,
    /// Destination folder.
    pub parent_entry_id: Uuid,
    /// Represented owner kind.
    pub owner_type: String,
    /// Represented owner identifier.
    pub owner_id: String,
    /// IAM-verified originating application.
    pub origin_app_id: Option<String>,
    /// Final entry name.
    pub name: String,
    /// Declared media type.
    pub content_type: String,
    /// Declared complete byte size.
    pub declared_size_bytes: i64,
    /// Calculated target part size.
    pub part_size_bytes: i64,
    /// Exact expected part count.
    pub expected_part_count: i32,
    /// Platform or organization storage selector.
    pub storage_backend: String,
    /// BYO storage configuration snapshot reference.
    pub storage_config_id: Option<Uuid>,
    /// Exact target bucket.
    pub bucket_name: String,
    /// Region snapshotted when the multipart session was created.
    pub storage_region: String,
    /// Tenant prefix snapshotted when the multipart session was created.
    pub storage_prefix: String,
    /// Server-side encryption mode snapshotted for the object.
    pub storage_encryption_mode: String,
    /// KMS key snapshotted for SSE-KMS objects.
    pub storage_kms_key_arn: Option<String>,
    /// Opaque final object key.
    pub object_key: String,
    /// Provider multipart identifier.
    pub provider_upload_id: String,
    /// Upload lifecycle state.
    pub status: String,
    /// Published entry after completion.
    pub completed_entry_id: Option<Uuid>,
    /// Automatic-abort deadline.
    pub expires_at: OffsetDateTime,
    /// Successful completion timestamp.
    pub completed_at: Option<OffsetDateTime>,
    /// Explicit abort timestamp.
    pub aborted_at: Option<OffsetDateTime>,
    /// Session creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// One uploaded provider multipart part.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct MultipartPartRow {
    /// Owning organization.
    pub org_id: String,
    /// Owning multipart session.
    pub upload_id: Uuid,
    /// One-based part number.
    pub part_number: i32,
    /// Exact provider entity tag.
    pub etag: String,
    /// Confirmed part byte count.
    pub size_bytes: i64,
    /// Raw 32-byte SHA-256 digest verified for the part.
    pub checksum_sha256: Vec<u8>,
    /// Last successful upload time for this part number.
    pub uploaded_at: OffsetDateTime,
}

/// Durable replay state for an externally initiated mutation.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct IdempotencyRecordRow {
    /// Owning organization.
    pub org_id: String,
    /// Represented actor kind.
    pub actor_type: String,
    /// Represented actor identifier.
    pub actor_id: String,
    /// Empty for bearer requests or the verified application identifier.
    pub origin_app_id: String,
    /// Stable operation name.
    pub operation: String,
    /// Client idempotency key.
    pub idempotency_key: String,
    /// Canonical request SHA-256 digest.
    pub request_hash: Vec<u8>,
    /// In-progress or completed state.
    pub status: String,
    /// Replay HTTP status.
    pub response_status: Option<i16>,
    /// Safe replay response headers.
    pub response_headers: Option<Value>,
    /// Safe replay JSON body.
    pub response_body: Option<Value>,
    /// Primary resource created by the operation.
    pub resource_id: Option<Uuid>,
    /// Deadline after which an abandoned claimant may be replaced.
    pub locked_until: OffsetDateTime,
    /// Retention deadline.
    pub expires_at: OffsetDateTime,
    /// First claim timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Redacted entry audit record.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct AuditEventRow {
    /// Owning organization.
    pub org_id: String,
    /// Audit event identifier.
    pub audit_id: Uuid,
    /// Related entry when the action targets one.
    pub entry_id: Option<Uuid>,
    /// Represented actor kind.
    pub actor_type: String,
    /// Represented actor identifier.
    pub actor_id: String,
    /// IAM-verified originating application.
    pub origin_app_id: Option<String>,
    /// Stable action name.
    pub action: String,
    /// Request correlation identifier.
    pub request_id: String,
    /// Redacted structured metadata.
    pub metadata: Value,
    /// Event timestamp.
    pub occurred_at: OffsetDateTime,
}

/// Transactional outbox event and worker lease state.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct OutboxEventRow {
    /// Owning organization.
    pub org_id: String,
    /// Event identifier.
    pub event_id: Uuid,
    /// Delivery topic.
    pub topic: String,
    /// Aggregate type.
    pub aggregate_type: String,
    /// Aggregate identifier.
    pub aggregate_id: String,
    /// Optional monotonic aggregate version.
    pub aggregate_version: Option<i64>,
    /// Versioned event payload.
    pub payload: Value,
    /// Pending, processing, delivered, or dead-letter state.
    pub status: String,
    /// Number of delivery claims.
    pub attempt_count: i32,
    /// Earliest next claim time.
    pub available_at: OffsetDateTime,
    /// Current worker lease token.
    pub lease_token: Option<Uuid>,
    /// Current worker lease deadline.
    pub lease_expires_at: Option<OffsetDateTime>,
    /// Redacted latest failure detail.
    pub last_error: Option<String>,
    /// Successful delivery timestamp.
    pub delivered_at: Option<OffsetDateTime>,
    /// Event creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Filename and extracted-content search projection.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct SearchDocumentRow {
    /// Owning organization.
    pub org_id: String,
    /// Indexed file entry.
    pub entry_id: Uuid,
    /// Current filename.
    pub filename: String,
    /// Extracted UTF-8 content, when supported.
    pub extracted_content: Option<String>,
    /// Extraction lifecycle state.
    pub extraction_status: String,
    /// Stable extraction failure code.
    pub extraction_error_code: Option<String>,
    /// Successful indexing timestamp.
    pub indexed_at: Option<OffsetDateTime>,
    /// Projection creation timestamp.
    pub created_at: OffsetDateTime,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Deduplicated inbound IAM webhook receipt.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct WebhookReceiptRow {
    /// Event source name.
    pub source: String,
    /// Source event identifier.
    pub event_id: String,
    /// Affected organization.
    pub org_id: String,
    /// Versioned event type.
    pub event_type: String,
    /// Projected aggregate type.
    pub aggregate_type: String,
    /// Projected aggregate identifier.
    pub aggregate_id: String,
    /// Monotonic source aggregate version.
    pub aggregate_version: i64,
    /// Signed source timestamp.
    pub signature_timestamp: OffsetDateTime,
    /// Raw request-body SHA-256 digest.
    pub payload_sha256: Vec<u8>,
    /// Receipt processing state.
    pub status: String,
    /// Stable redacted failure code.
    pub failure_code: Option<String>,
    /// Receipt timestamp.
    pub received_at: OffsetDateTime,
    /// Terminal processing timestamp.
    pub processed_at: Option<OffsetDateTime>,
    /// Last state-change timestamp.
    pub updated_at: OffsetDateTime,
}

/// Expands to the exact `briefcase.entries` column list decoded into
/// [`EntryRow`], so every projection stays in sync with the row type.
macro_rules! entry_columns {
    () => {
        "org_id, entry_id, parent_id, entry_type, name, path, root_type, tag_id, \
         system_kind, owner_type, owner_id, origin_app_id, content_type, size_bytes, \
         current_version_id, created_by_type, created_by_id, updated_by_type, \
         updated_by_id, deletion_batch_id, deleted_at, purge_after, created_at, updated_at"
    };
}

pub(super) use entry_columns;
