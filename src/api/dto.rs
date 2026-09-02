//! Public HTTP data-transfer objects from `openapi.yaml`.
//!
//! These types intentionally do not contain authorization behavior. Handlers
//! validate them and map them into domain commands before invoking a use case.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

/// IAM principal kind accepted by the Briefcase API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorTypeDto {
    /// Human account.
    Carbon,
    /// AI-agent account.
    Silicon,
    /// Registered application principal.
    Application,
}

/// Public reference to an IAM principal.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ActorRefDto {
    /// Principal kind.
    #[serde(rename = "type")]
    pub actor_type: ActorTypeDto,
    /// IAM principal identifier.
    pub id: String,
}

/// File or folder discriminator.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryTypeDto {
    /// Content-bearing filesystem entry.
    File,
    /// Container filesystem entry.
    Folder,
}

/// Inherited visibility boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootTypeDto {
    /// Readable by every current organization member.
    Public,
    /// Readable only through ownership or explicit authority.
    Private,
    /// Readable by current members of one IAM tag.
    Tag,
}

/// Effective capability exposed by entry responses.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveAccessDto {
    /// Read metadata and content.
    Read,
    /// Add content that does not exist yet.
    Write,
    /// Change an entry that already exists.
    Update,
    /// Move the entry to the recoverable bin.
    Delete,
    /// Grant and revoke explicit permissions.
    ManagePermissions,
}

/// Renderer a client should open for a file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderKindDto {
    /// Detail image view.
    Image,
    /// Video player.
    Video,
    /// In-place document view.
    Document,
    /// Sheet view.
    Spreadsheet,
    /// Presentation view.
    Presentation,
    /// Audio player.
    Audio,
    /// Archive content listing without extraction.
    Archive,
    /// Syntax-highlighted code or data view.
    Code,
    /// No renderer applies.
    Unsupported,
}

/// Entry representation returned by the public API.
#[derive(Clone, Debug, Serialize)]
pub struct EntryDto {
    /// Stable entry identifier.
    pub id: Uuid,
    /// Organization security boundary.
    pub org_id: String,
    /// Entry discriminator.
    #[serde(rename = "type")]
    pub entry_type: EntryTypeDto,
    /// Display name within the parent.
    pub name: String,
    /// Organization-relative path, without a leading separator.
    pub path: String,
    /// Parent folder, or `None` at organization root.
    pub parent_id: Option<Uuid>,
    /// Inherited visibility boundary.
    pub root_type: RootTypeDto,
    /// IAM tag backing a tag boundary.
    pub tag: Option<String>,
    /// Media type for a file.
    pub content_type: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Client renderer for a file.
    pub render: Option<RenderKindDto>,
    /// Clean permanent URL that shows the folder structure.
    pub permanent_url: Url,
    /// Authenticated sandboxed content URL for a file.
    pub content_url: Option<Url>,
    /// Authenticated attachment URL for a file.
    pub download_url: Option<Url>,
    /// Represented actor who owns the entry.
    pub owner: ActorRefDto,
    /// Verified application that originated the entry.
    pub origin_app_id: Option<String>,
    /// Capabilities effective for the current caller.
    pub effective_access: Vec<EffectiveAccessDto>,
    /// Creation timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last metadata or content update timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Recoverable deletion timestamp.
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

/// Cursor-paginated entry response.
#[derive(Clone, Debug, Serialize)]
pub struct EntryPageDto {
    /// Visible entries in stable order.
    pub items: Vec<EntryDto>,
    /// Opaque cursor for the next page.
    pub next_cursor: Option<String>,
}

/// Query parameters for listing entries.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListEntriesQuery {
    /// Parent folder; omission lists organization roots.
    pub parent_id: Option<Uuid>,
    /// Opaque pagination cursor.
    pub cursor: Option<String>,
    /// Page size from 1 through 100.
    pub limit: Option<u16>,
}

/// Folder creation request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FolderCreateDto {
    /// Display name.
    pub name: String,
    /// Destination folder, or organization root when omitted.
    pub parent_id: Option<Uuid>,
    /// Required only for organization-root creation.
    pub root_type: Option<RootTypeDto>,
    /// Required for a tag root.
    pub tag: Option<String>,
    /// Explicit grants created atomically with the folder.
    #[serde(default)]
    pub invitees: Vec<PermissionGrantCreateDto>,
}

/// Entry metadata update request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryPatchDto {
    /// Replacement display name.
    pub name: Option<String>,
    /// Replacement parent folder.
    pub parent_id: Option<Uuid>,
}

/// Whether a permanent URL should return bytes for rendering or downloading.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispositionDto {
    /// Stream the bytes for in-place rendering.
    Inline,
    /// Stream the bytes as a local download.
    Attachment,
}

/// Query parameters accepted by the clean permanent URL.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathContentQuery {
    /// Omission returns entry metadata instead of content.
    pub disposition: Option<DispositionDto>,
}

/// Multipart-upload initialization request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MultipartUploadCreateDto {
    /// Destination folder.
    pub parent_id: Uuid,
    /// Final file name.
    pub name: String,
    /// Declared final size in bytes.
    pub size: u64,
    /// Declared media type.
    pub content_type: String,
}

/// Multipart-upload initialization response.
#[derive(Clone, Debug, Serialize)]
pub struct MultipartUploadDto {
    /// Briefcase upload identifier.
    pub upload_id: Uuid,
    /// Required non-final part size.
    pub part_size: u64,
    /// Expected number of parts.
    pub part_count: u32,
    /// Session expiry.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// Client-reported completed S3 part.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPartDto {
    /// One-based part number.
    pub part_number: u32,
    /// Exact `ETag` returned by the part endpoint.
    pub etag: String,
}

/// Multipart completion request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MultipartCompleteDto {
    /// Exact ordered part list.
    pub parts: Vec<CompletedPartDto>,
}

/// One right conveyed by an invitation or requested through the workflow.
///
/// The rights are independent: `update` does not imply `delete`, and `write`
/// does not imply `update`. Every set implicitly includes `read`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantAccessDto {
    /// Metadata and content read access.
    Read,
    /// Add children to a folder or content to a file.
    Write,
    /// Rename, move, or replace what already exists.
    Update,
    /// Move the entry to the recoverable bin.
    Delete,
}

/// Permission-grant creation request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionGrantCreateDto {
    /// Carbon or Silicon receiving authority.
    pub principal: ActorRefDto,
    /// Non-empty normalized access levels.
    pub access: Vec<GrantAccessDto>,
    /// Whether authority reaches descendants.
    #[serde(default = "default_true")]
    pub inherit: bool,
}

/// Explicit permission grant response.
#[derive(Clone, Debug, Serialize)]
pub struct PermissionGrantDto {
    /// Grant identifier.
    pub id: Uuid,
    /// Carbon or Silicon receiving authority.
    pub principal: ActorRefDto,
    /// Granted levels.
    pub access: Vec<GrantAccessDto>,
    /// Whether authority reaches descendants.
    pub inherit: bool,
    /// Actor who created the grant.
    pub granted_by: ActorRefDto,
    /// Creation timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Permission grant collection.
#[derive(Clone, Debug, Serialize)]
pub struct PermissionGrantPageDto {
    /// Explicit grants on the requested entry.
    pub items: Vec<PermissionGrantDto>,
}

/// Batch request for the caller's own effective access.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionInspectionDto {
    /// Targets addressed by identifier.
    #[serde(default)]
    pub entry_ids: Vec<Uuid>,
    /// Targets addressed by organization-relative path.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// What the caller may do on one inspected target.
#[derive(Clone, Debug, Serialize)]
pub struct EffectivePermissionDto {
    /// Resolved entry identifier.
    pub entry_id: Uuid,
    /// Resolved organization-relative path.
    pub path: String,
    /// File or folder discriminator.
    #[serde(rename = "type")]
    pub entry_type: EntryTypeDto,
    /// Everything the caller may do on this entry.
    pub effective_access: Vec<EffectiveAccessDto>,
}

/// Effective-access answer for a batch of targets.
#[derive(Clone, Debug, Serialize)]
pub struct PermissionInspectionResultDto {
    /// One item per readable target, ordered by path.
    pub items: Vec<EffectivePermissionDto>,
    /// Requested identifiers with no readable entry.
    pub unresolved_entry_ids: Vec<Uuid>,
    /// Requested paths with no readable entry.
    pub unresolved_paths: Vec<String>,
}

/// Access-request creation payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestCreateDto {
    /// Non-empty set of desired rights.
    pub access: Vec<GrantAccessDto>,
    /// Optional human-readable context.
    pub reason: Option<String>,
}

/// Access-request status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRequestStatusDto {
    /// Awaiting an eligible decision.
    Pending,
    /// Approved and materialized as a grant.
    Approved,
    /// Denied without creating authority.
    Denied,
}

/// Access-request response.
#[derive(Clone, Debug, Serialize)]
pub struct AccessRequestDto {
    /// Request identifier.
    pub id: Uuid,
    /// Target entry.
    pub entry_id: Uuid,
    /// Requesting actor.
    pub requested_by: ActorRefDto,
    /// Requested rights.
    pub access: Vec<GrantAccessDto>,
    /// Current workflow status.
    pub status: AccessRequestStatusDto,
    /// Creation timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Access-request decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessDecisionDto {
    /// Approve and create a grant.
    Approve,
    /// Deny without creating a grant.
    Deny,
}

/// Access-request decision payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequestDecisionDto {
    /// Terminal decision.
    pub decision: AccessDecisionDto,
    /// Granted rights; required on approval and forbidden on denial.
    pub access: Option<Vec<GrantAccessDto>>,
}

/// File search query.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchQueryDto {
    /// Non-empty user query.
    pub q: String,
    /// Maximum result count from 1 through 20.
    pub limit: Option<u8>,
}

/// Permission-filtered search hit.
#[derive(Clone, Debug, Serialize)]
pub struct SearchResultDto {
    /// Visible entry.
    pub entry: EntryDto,
    /// Stable ranking score within this response.
    pub score: f64,
    /// Whether the filename matched.
    pub filename_match: bool,
    /// Number of content matches.
    pub content_hits: u32,
    /// Safe excerpts, when extraction supports them.
    pub snippets: Vec<String>,
}

/// Search response.
#[derive(Clone, Debug, Serialize)]
pub struct SearchPageDto {
    /// Ranked search results.
    pub items: Vec<SearchResultDto>,
}

/// Retained file version.
#[derive(Clone, Debug, Serialize)]
pub struct FileVersionDto {
    /// Version identifier.
    pub id: Uuid,
    /// Monotonic per-file number.
    pub number: u32,
    /// Content size in bytes.
    pub size: u64,
    /// Actor who created this version.
    pub created_by: ActorRefDto,
    /// Creation timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// File version collection.
#[derive(Clone, Debug, Serialize)]
pub struct FileVersionPageDto {
    /// Newest-first versions, capped at 50.
    pub items: Vec<FileVersionDto>,
}

/// Server-side encryption mode for an organization bucket.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionModeDto {
    /// Amazon S3-managed keys.
    SseS3,
    /// Customer-selected AWS KMS key.
    SseKms,
}

/// Organization-owned S3 configuration request.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BucketConfigurationDto {
    /// S3 bucket name.
    pub bucket_name: String,
    /// AWS region.
    pub region: String,
    /// Cross-account IAM role assumed by Briefcase.
    pub role_arn: String,
    /// Organization-owned prefix.
    pub prefix: String,
    /// Expected AWS account ID.
    pub aws_account_id: String,
    /// Required encryption behavior.
    pub encryption_mode: EncryptionModeDto,
    /// Required when `encryption_mode` is `sse_kms`.
    pub kms_key_arn: Option<String>,
}

/// Storage validation outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketConfigurationStateDto {
    /// Probe succeeded and the configuration became active.
    Configured,
    /// Probe failed and the previous configuration remains active.
    Failed,
}

/// Organization storage configuration response.
#[derive(Clone, Debug, Serialize)]
pub struct BucketConfigurationStatusDto {
    /// Validation outcome.
    pub status: BucketConfigurationStateDto,
    /// Probe completion time.
    #[serde(with = "time::serde::rfc3339")]
    pub tested_at: OffsetDateTime,
    /// Redacted validation failure category.
    pub failure_reason: Option<String>,
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{GrantAccessDto, PermissionGrantCreateDto};

    #[test]
    fn permission_inheritance_defaults_to_true() {
        let json = r#"{
            "principal": { "type": "carbon", "id": "carbon-a" },
            "access": ["read"]
        }"#;
        let parsed = serde_json::from_str::<PermissionGrantCreateDto>(json);
        assert!(matches!(
            parsed,
            Ok(PermissionGrantCreateDto {
                inherit: true,
                access,
                ..
            }) if access == vec![GrantAccessDto::Read]
        ));
    }
}
