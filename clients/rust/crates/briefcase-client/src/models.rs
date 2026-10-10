//! The shapes Briefcase sends and accepts.
//!
//! These mirror the published contract. Anything Briefcase treats as opaque —
//! cursors, proofs, idempotency keys — stays opaque here too. Accounts are
//! keyed by their permanent `uuid` (short, case-sensitive text such as `zQo`,
//! never an RFC 4122 UUID); their `c:`/`si:` id is for display and can change.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

/// Kind of principal an action or grant names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// A Carbon.
    Carbon,
    /// A Silicon.
    Silicon,
    /// An app acting through a proof.
    Application,
}

impl ActorType {
    /// Returns the wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Carbon => "carbon",
            Self::Silicon => "silicon",
            Self::Application => "application",
        }
    }
}

impl std::fmt::Display for ActorType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A reference to one Carbon or Silicon.
///
/// When a request names an account (a grant, an invitee), `id` is its `c:`/`si:`
/// id, such as `c:ada`. In Briefcase's answers (owners, actors, grantees) `id`
/// is the account's permanent uuid, because ids can change.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ActorRef {
    /// Principal kind.
    #[serde(rename = "type")]
    pub actor_type: ActorType,
    /// A `c:`/`si:` id in requests, the account's uuid in answers.
    pub id: String,
}

impl ActorRef {
    /// Names a Carbon, such as `c:ada`.
    #[must_use]
    pub fn carbon(id: impl Into<String>) -> Self {
        Self {
            actor_type: ActorType::Carbon,
            id: id.into(),
        }
    }

    /// Names a Silicon, such as `si:scout`.
    #[must_use]
    pub fn silicon(id: impl Into<String>) -> Self {
        Self {
            actor_type: ActorType::Silicon,
            id: id.into(),
        }
    }

    /// Names an account by its `c:` or `si:` id, taking the kind from the prefix.
    ///
    /// Returns `None` for anything that is not a `c:`/`si:` id.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        let (kind, handle) = id.split_once(':')?;
        if handle.is_empty() || handle.contains(['/', ':']) || handle.contains(char::is_whitespace)
        {
            return None;
        }
        match kind {
            "c" => Some(Self::carbon(id)),
            "si" => Some(Self::silicon(id)),
            _ => None,
        }
    }
}

impl std::fmt::Display for ActorRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.id)
    }
}

/// Whether an account is a Carbon or a Silicon.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// A Carbon (`c:` ids).
    Carbon,
    /// A Silicon (`si:` ids).
    Silicon,
}

impl AccountKind {
    /// Returns the wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Carbon => "carbon",
            Self::Silicon => "silicon",
        }
    }
}

impl std::fmt::Display for AccountKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Another account, by its permanent uuid and (when known) its current id.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AccountRef {
    /// The permanent, case-sensitive account identifier.
    pub uuid: String,
    /// The current public id, such as `c:ada`; it can change.
    #[serde(default)]
    pub id: Option<String>,
}

/// An account as Briefcase shows it next to a drive or a share.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountSummary {
    /// The permanent, case-sensitive account identifier.
    pub uuid: String,
    /// The current public id, such as `c:ada` or `si:scout`; it can change.
    #[serde(default)]
    pub id: Option<String>,
    /// Carbon or Silicon.
    pub kind: AccountKind,
    /// The display name, when Briefcase knows it.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The profile photo, when Briefcase knows it.
    #[serde(default)]
    pub pfp_url: Option<String>,
}

impl AccountSummary {
    /// The public id, or the uuid while Briefcase has not learned it.
    #[must_use]
    pub fn display_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.uuid)
    }
}

/// The signed-in account, verified by Briefcase (`GET /api/v1/me`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Account {
    /// The permanent, case-sensitive account identifier.
    pub uuid: String,
    /// The current public id; it can change.
    #[serde(default)]
    pub id: Option<String>,
    /// Carbon or Silicon.
    pub kind: AccountKind,
    /// The display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The profile photo.
    #[serde(default)]
    pub pfp_url: Option<String>,
    /// A Silicon's custodian (a Carbon); absent for Carbons.
    #[serde(default)]
    pub custodian: Option<AccountRef>,
}

impl Account {
    /// The public id, or the uuid while Briefcase has not learned it.
    #[must_use]
    pub fn display_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.uuid)
    }
}

/// How this deployment signs accounts in (`GET /api/v1/accounts`, no
/// credential needed).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AccountsInfo {
    /// Briefcase's app id at Silicon Accounts, the audience of its access tokens.
    pub app_id: String,
    /// The Silicon Accounts URL, also the issuer of the access tokens.
    pub accounts_url: String,
    /// The scopes another app may hold in a User verification proof to act for
    /// an account here.
    #[serde(default)]
    pub delegated_scopes: Vec<String>,
}

/// How the caller relates to a drive it opens with the owner's rights.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveRelation {
    /// The caller's own drive.
    Own,
    /// The drive of a Silicon the caller looks after.
    Custodial,
}

/// A drive the caller opens with its owner's rights: its own, or a Silicon's
/// it looks after.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Drive {
    /// Whose drive it is.
    pub owner: AccountSummary,
    /// Own, or a Silicon the caller looks after.
    pub relation: DriveRelation,
    /// The drive's top folder; list it with `ListEntries::in_folder(root_id)`.
    pub root_id: Uuid,
    /// The drive's permanent address, such as `https://briefcase.teamofsilicons.com/c:ada`.
    pub permanent_url: Url,
}

/// The drives the caller opens with the owner's rights.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DriveList {
    /// The caller's own drive first, then one per Silicon it looks after.
    pub items: Vec<Drive>,
}

/// Something another account shared with the caller.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SharedEntry {
    /// Whose drive the entry is in.
    pub owner: AccountSummary,
    /// When the newest live share on the entry was made.
    #[serde(with = "time::serde::rfc3339")]
    pub shared_at: OffsetDateTime,
    /// The entry.
    pub entry: Entry,
}

/// One page of Shared with me.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SharedPage {
    /// Newest share first, one item per entry.
    pub items: Vec<SharedEntry>,
    /// Cursor for the following page, absent at the end.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// One account a Silicon (or its custodian) allowed to share with it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AllowedAccount {
    /// The allowed account.
    pub account: AccountSummary,
    /// When it was allowed.
    #[serde(with = "time::serde::rfc3339")]
    pub allowed_at: OffsetDateTime,
}

/// The accounts outside a Silicon's custodian and the custodian's other
/// Silicons that may share with it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AllowedAccounts {
    /// The Silicon.
    pub silicon: AccountSummary,
    /// The accounts it allows.
    pub items: Vec<AllowedAccount>,
}

/// Whether an entry stores bytes or contains other entries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    /// A file.
    File,
    /// A folder.
    Folder,
}

/// How much of an entry the caller can see.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryVisibility {
    /// Every field is populated.
    Full,
    /// A folder reachable only because something inside it was shared.
    Traversal,
}

/// The renderer a client should open for a file.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderKind {
    /// Still image.
    Image,
    /// Moving image.
    Video,
    /// Paginated or prose document.
    Document,
    /// Tabular data.
    Spreadsheet,
    /// Slide deck.
    Presentation,
    /// Sound.
    Audio,
    /// Container listed without extraction.
    Archive,
    /// Source or structured data.
    Code,
    /// No renderer applies.
    Unsupported,
}

/// What the caller may do with an entry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveAccess {
    /// Read metadata and content.
    Read,
    /// Add content that does not exist yet.
    Write,
    /// Change content that already exists.
    Update,
    /// Move the entry to the recoverable bin.
    Delete,
    /// Grant and revoke access.
    ManagePermissions,
}

impl EffectiveAccess {
    /// Returns the wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::ManagePermissions => "manage_permissions",
        }
    }
}

impl std::fmt::Display for EffectiveAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One right a share conveys. Delete is never shared: only the creator, the
/// drive's owner and their custodians may delete.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRight {
    /// View and download.
    Read,
    /// Add new content.
    Write,
    /// Change what already exists.
    Update,
    /// Move to the recoverable bin (never grantable; kept so older answers parse).
    Delete,
}

impl AccessRight {
    /// Returns the wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    /// Parses a right from its wire spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

impl std::fmt::Display for AccessRight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A file or folder.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    /// Stable identifier.
    pub id: Uuid,
    /// The drive the entry is in: its owner's account uuid.
    ///
    /// Briefcase's API names this field `org_id`; it is read from there and
    /// written as `drive`.
    #[serde(rename(serialize = "drive"), alias = "org_id")]
    pub drive: String,
    /// File or folder.
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    /// How much of this entry is disclosed.
    pub visibility: EntryVisibility,
    /// Display name within the parent.
    pub name: String,
    /// Path within the drive, without a leading separator.
    pub path: String,
    /// Parent folder, absent at the top of a drive.
    pub parent_id: Option<Uuid>,
    /// Media type of a file.
    pub content_type: Option<String>,
    /// Size of a file in bytes.
    pub size: Option<u64>,
    /// Renderer a client should open for a file.
    pub render: Option<RenderKind>,
    /// Clean permanent URL below the drive owner's id, such as
    /// `https://briefcase.teamofsilicons.com/c:ada/notes/plan.md`.
    pub permanent_url: Url,
    /// Authenticated sandboxed content URL for a file.
    pub content_url: Option<Url>,
    /// Authenticated attachment URL for a file.
    pub download_url: Option<Url>,
    /// Creator, absent for a traversal folder and for a system folder (the top
    /// of a drive, `apps/`, `apps/{app}/`).
    pub owner: Option<ActorRef>,
    /// App that created the entry through a proof, such as `waveform`.
    pub origin_app_id: Option<String>,
    /// What the caller may do here.
    pub effective_access: Vec<EffectiveAccess>,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    /// Last change time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// Time the entry was moved to the bin.
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
    /// When a self-destructing file is permanently deleted; absent for a
    /// permanent file and every folder.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub self_destruct_at: Option<OffsetDateTime>,
}

impl Entry {
    /// Returns whether the entry is a folder.
    #[must_use]
    pub const fn is_folder(&self) -> bool {
        matches!(self.entry_type, EntryType::Folder)
    }

    /// Returns whether the caller holds a right on this entry.
    #[must_use]
    pub fn allows(&self, access: EffectiveAccess) -> bool {
        self.effective_access.contains(&access)
    }
}

/// One page of entries.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EntryPage {
    /// Visible entries.
    pub items: Vec<Entry>,
    /// Cursor for the following page, absent at the end.
    pub next_cursor: Option<String>,
}

/// An explicit share (permission grant) on an entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PermissionGrant {
    /// Grant identifier.
    pub id: Uuid,
    /// The account the grant is for (its uuid in `id`).
    pub principal: ActorRef,
    /// Rights it conveys.
    pub access: Vec<AccessRight>,
    /// Whether it reaches descendants.
    pub inherit: bool,
    /// Who created it.
    pub granted_by: ActorRef,
    /// When it was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When this expiring share ends; absent for a permanent grant.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

/// Explicit grants on one entry.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PermissionGrantPage {
    /// The grants.
    pub items: Vec<PermissionGrant>,
}

/// What the caller may do on one inspected target.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EffectivePermission {
    /// Resolved entry identifier.
    pub entry_id: Uuid,
    /// Resolved path.
    pub path: String,
    /// File or folder.
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    /// Everything the caller may do there.
    pub effective_access: Vec<EffectiveAccess>,
}

/// The answer to "what may I do with these?".
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PermissionInspection {
    /// One item per readable target.
    pub items: Vec<EffectivePermission>,
    /// Identifiers with no readable entry.
    pub unresolved_entry_ids: Vec<Uuid>,
    /// Paths with no readable entry.
    pub unresolved_paths: Vec<String>,
}

/// What a notification is about.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// The recipient received access.
    AccessGranted,
    /// The recipient's access was revoked.
    AccessRevoked,
    /// A request awaits the recipient's decision.
    AccessRequested,
    /// The recipient's own request was decided.
    AccessRequestDecided,
}

/// How a request was decided.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDecision {
    /// Approved.
    Approved,
    /// Denied.
    Denied,
}

/// The entry a notification refers to, as it was at that moment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NotificationSubject {
    /// Entry identifier.
    pub entry_id: Uuid,
    /// The drive holding the entry (its owner's uuid); absent on notifications
    /// from before personal drives.
    #[serde(default)]
    pub drive: Option<String>,
    /// Name at the time.
    pub name: String,
    /// Path at the time.
    pub path: String,
    /// File or folder.
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    /// Permanent URL of the entry.
    pub permanent_url: Url,
}

/// One notification.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Notification {
    /// Notification identifier.
    pub id: Uuid,
    /// What happened.
    pub kind: NotificationKind,
    /// Whether it has been read.
    pub read: bool,
    /// Who acted.
    pub actor: Option<ActorRef>,
    /// What it refers to.
    pub subject: Option<NotificationSubject>,
    /// Rights involved.
    pub access: Option<Vec<AccessRight>>,
    /// Access request it belongs to.
    pub access_request_id: Option<Uuid>,
    /// Outcome of a decided request.
    pub decision: Option<NotificationDecision>,
    /// When it was written.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// The notification inbox and its badge count.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NotificationInbox {
    /// Twenty newest notifications, newest first.
    pub items: Vec<Notification>,
    /// Unread count for a badge.
    pub unread_count: u64,
}

/// One consumption figure against its limit, in bytes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct UsageMeasure {
    /// Bytes consumed.
    pub used_bytes: u64,
    /// Bytes allowed.
    pub limit_bytes: u64,
    /// Bytes still available.
    pub remaining_bytes: u64,
}

/// The day's upload consumption and when it returns.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct DailyUsageMeasure {
    /// Bytes uploaded today.
    pub used_bytes: u64,
    /// Bytes allowed in one UTC day.
    pub limit_bytes: u64,
    /// Bytes still available today.
    pub remaining_bytes: u64,
    /// Next midnight UTC.
    #[serde(with = "time::serde::rfc3339")]
    pub resets_at: OffsetDateTime,
}

/// What a drive is consuming. Uploads count against the drive they land in,
/// whoever uploads them.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct DriveUsage {
    /// Storage consumption and ceiling (1 PiB by default).
    pub storage: UsageMeasure,
    /// Upload volume within the current UTC day (100 GiB by default).
    pub daily_uploads: DailyUsageMeasure,
}

/// One search hit.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchResult {
    /// The entry found.
    pub entry: Entry,
    /// Ranking score within this response.
    pub score: f64,
    /// Whether the filename matched.
    pub filename_match: bool,
    /// Number of content matches.
    pub content_hits: u32,
    /// Excerpts, when extraction supports them.
    pub snippets: Vec<String>,
}

/// Search results.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchPage {
    /// Ranked hits.
    pub items: Vec<SearchResult>,
}

/// One recorded action in an entry's history.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActivityEvent {
    /// Stable action name, such as `entry.file_created.v1`.
    pub action: String,
    /// Who performed it.
    pub actor: ActorRef,
    /// App that acted for the account through a proof.
    pub app_id: Option<String>,
    /// When it happened.
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}

/// An entry's retained history.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActivityPage {
    /// Newest-first history, capped at a hundred records.
    pub items: Vec<ActivityEvent>,
}

/// One retained version of a file.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileVersion {
    /// Version identifier.
    pub id: Uuid,
    /// Monotonic version number.
    pub number: u64,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the complete file bytes.
    pub sha256: Option<String>,
    /// How the immutable version was created.
    pub source: String,
    /// Who created it.
    pub created_by: ActorRef,
    /// When it was created.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A file's retained versions.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileVersionPage {
    /// Newest-first versions, up to 100 per page.
    pub items: Vec<FileVersion>,
    /// Continue without losing access to older retained versions.
    pub next_cursor: Option<String>,
}

/// Server-side encryption for a bring-your-own bucket.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionMode {
    /// S3-managed keys.
    SseS3,
    /// A customer-selected KMS key.
    SseKms,
}

/// An S3 bucket the drive's owner brings for its files.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BucketConfiguration {
    /// The drive to configure (its owner's `c:`/`si:` id or uuid); the caller's
    /// own when absent. A custodian configures a Silicon's drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
    /// Bucket name.
    pub bucket_name: String,
    /// AWS region.
    pub region: String,
    /// Cross-account role Briefcase assumes.
    pub role_arn: String,
    /// Prefix inside the bucket.
    pub prefix: String,
    /// Expected AWS account.
    pub aws_account_id: String,
    /// Required encryption behavior.
    pub encryption_mode: EncryptionMode,
    /// KMS key, required when the mode is `sse_kms`.
    pub kms_key_arn: Option<String>,
}

/// Whether a storage configuration probe succeeded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BucketConfigurationState {
    /// The probe succeeded and the configuration is active.
    Configured,
    /// The probe failed and the previous configuration remains.
    Failed,
}

/// The outcome of configuring a drive's storage.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BucketConfigurationStatus {
    /// Outcome.
    pub status: BucketConfigurationState,
    /// When the probe completed.
    #[serde(with = "time::serde::rfc3339")]
    pub tested_at: OffsetDateTime,
    /// Redacted failure category, when it failed.
    pub failure_reason: Option<String>,
}

/// Liveness or readiness of a deployment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServiceStatus {
    /// `ok` for liveness, `ready` for readiness.
    pub status: String,
}

/// What one entry inside an archive is.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveItemType {
    /// A file.
    File,
    /// A folder.
    Folder,
    /// A symbolic or hard link.
    Link,
    /// Anything else.
    #[serde(other)]
    Other,
}

/// One entry inside an archive, listed without extracting it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchiveItem {
    /// The path inside the archive, as the archive spells it.
    pub path: String,
    /// File, folder, link or other.
    #[serde(rename = "type")]
    pub item_type: ArchiveItemType,
    /// Bytes once extracted.
    pub size: u64,
    /// Bytes inside the archive (zip only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compressed_size: Option<u64>,
    /// When the entry was last changed, when the archive records it.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_at: Option<OffsetDateTime>,
    /// Whether the entry's content is encrypted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
}

/// The contents of an archive file (zip, tar, tar.gz, tar.zst, 7z).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchiveListing {
    /// `zip`, `tar`, `tar.gz`, `tar.zst` or `7z`.
    pub format: String,
    /// What the archive holds, in the archive's order.
    pub entries: Vec<ArchiveItem>,
    /// Whether the archive holds more than is listed.
    pub truncated: bool,
    /// Why the listing stopped early: `entry_limit`, `directory_limit`,
    /// `read_limit` or `unreadable_rest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_because: Option<String>,
}

/// A permanent address whose owner id moved to another id (`308`).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Moved {
    /// The same API address under the owner's current id.
    pub moved_to: Url,
    /// The matching permanent address on the website.
    pub permanent_url: Url,
    /// The drive's owner.
    pub owner: AccountRef,
}

#[cfg(test)]
mod tests {
    use super::{
        AccessRight, ActorRef, ActorType, DriveList, DriveRelation, EffectiveAccess, Entry,
        SharedPage,
    };

    #[test]
    fn an_actor_reads_as_kind_and_identifier() {
        let actor = ActorRef::carbon("c:ada");
        assert_eq!(actor.actor_type, ActorType::Carbon);
        assert_eq!(actor.to_string(), "c:ada");
        assert_eq!(
            ActorRef::from_id("si:scout"),
            Some(ActorRef::silicon("si:scout"))
        );
        assert_eq!(ActorRef::from_id("email:a@b.c"), None);
        assert_eq!(ActorRef::from_id("c:"), None);
        assert_eq!(ActorRef::from_id("c:a/b"), None);
    }

    #[test]
    fn rights_round_trip_through_their_wire_spelling() {
        for right in [
            AccessRight::Read,
            AccessRight::Write,
            AccessRight::Update,
            AccessRight::Delete,
        ] {
            assert_eq!(AccessRight::parse(right.as_str()), Some(right));
        }
        assert_eq!(AccessRight::parse(" READ "), Some(AccessRight::Read));
        assert_eq!(AccessRight::parse("manage_permissions"), None);
    }

    fn entry_json() -> &'static str {
        r#"{
            "id": "01a067ce-7f19-7790-820a-0be6b3d4f828",
            "org_id": "zQo",
            "type": "file",
            "visibility": "full",
            "name": "plan.md",
            "path": "notes/plan.md",
            "parent_id": null,
            "root_type": "private",
            "tag": null,
            "content_type": "text/markdown",
            "size": 12,
            "render": "document",
            "permanent_url": "https://briefcase.example/c:ada/notes/plan.md",
            "content_url": null,
            "download_url": null,
            "owner": {"type": "carbon", "id": "zQo"},
            "origin_app_id": null,
            "effective_access": ["read", "update"],
            "created_at": null,
            "updated_at": null,
            "deleted_at": null
        }"#
    }

    #[test]
    fn an_entry_names_its_drive_and_what_the_caller_may_do() {
        let entry: Entry =
            serde_json::from_str(entry_json()).expect("the contract's entry shape must parse");
        assert_eq!(entry.drive, "zQo");
        assert!(!entry.is_folder());
        assert!(entry.allows(EffectiveAccess::Update));
        assert!(!entry.allows(EffectiveAccess::Delete));
        // Written back as `drive`, and read again from either name.
        let written = serde_json::to_value(&entry).unwrap();
        assert_eq!(written["drive"], "zQo");
        assert!(written.get("org_id").is_none());
        let again: Entry = serde_json::from_value(written).unwrap();
        assert_eq!(again.drive, "zQo");
    }

    #[test]
    fn drives_and_shared_pages_parse() {
        let drives: DriveList = serde_json::from_str(
            r#"{"items":[
                {"owner":{"uuid":"zQo","id":"c:ada","kind":"carbon","display_name":"Ada","pfp_url":null},
                 "relation":"own","root_id":"01a067ce-7f19-7790-820a-0be6b3d4f801",
                 "permanent_url":"https://briefcase.example/c:ada"},
                {"owner":{"uuid":"Sc0","id":null,"kind":"silicon"},
                 "relation":"custodial","root_id":"01a067ce-7f19-7790-820a-0be6b3d4f802",
                 "permanent_url":"https://briefcase.example/Sc0"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(drives.items[0].relation, DriveRelation::Own);
        assert_eq!(drives.items[1].owner.display_id(), "Sc0");
        let shared: SharedPage = serde_json::from_str(&format!(
            r#"{{"items":[{{"owner":{{"uuid":"zQo","id":"c:ada","kind":"carbon"}},
                "shared_at":"2026-10-10T10:00:00Z","entry":{}}}],"next_cursor":null}}"#,
            entry_json()
        ))
        .unwrap();
        assert_eq!(shared.items[0].owner.display_id(), "c:ada");
        assert_eq!(shared.items[0].entry.name, "plan.md");
    }
}
