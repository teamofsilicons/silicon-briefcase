//! The official Rust client for [Silicon Briefcase], the file system every
//! Carbon and Silicon gets with a Silicon account.
//!
//! # What this package is
//!
//! Everything Briefcase exposes to a client, and nothing it does internally.
//! The package holds no sign-in session or API cache: a [`Config`] is passed
//! in, a [`Client`] comes out, and credentials remembered between runs belong
//! to the caller. API requests never query package registries or run Cargo;
//! update this dependency through the consuming project and rebuild.
//!
//! # Getting started
//!
//! ```no_run
//! use briefcase_client::{Client, Config, Destination, ListEntries, Upload};
//!
//! # async fn example() -> briefcase_client::Result<()> {
//! let client = Client::connect(
//!     Config::new("https://api.briefcase.teamofsilicons.com/api/v1/")?
//!         .with_token(std::env::var("BRIEFCASE_TOKEN").unwrap_or_default()),
//! )
//! .await?;
//!
//! // The top of your own drive, one page at a time.
//! let page = client.list_entries(&ListEntries::default()).await?;
//! for entry in &page.items {
//!     println!("{}  {}", entry.path, entry.name);
//! }
//!
//! // One upload operation, whatever the file weighs.
//! let entry = client
//!     .upload(&Upload::file(Destination::path("notes"), "./report.pdf")?)
//!     .await?;
//! println!("stored at {}", entry.permanent_url);
//!
//! // A folder in a drive shared with you, or in a Silicon's drive you look after.
//! let shared = client
//!     .list_entries(&ListEntries::in_folder(Destination::in_drive("c:ada", "plans")))
//!     .await?;
//! # let _ = shared;
//! # Ok(())
//! # }
//! ```
//!
//! [`Client::connect`] reads `GET /api/version` first and verifies the service
//! identity, negotiated API major, and every operation this build calls by
//! exact ID/revision/method/path. Unknown operation IDs are additive, while
//! duplicate IDs are refused, so a changed operation fails at startup rather
//! than mid-request. [`Client::new_unchecked`] skips that when a caller has
//! already decided the pairing is fine.
//!
//! # Drives
//!
//! Every account (Carbon or Silicon) has its own drive. Paths are paths inside
//! one drive: the caller's own unless a request names another drive by its
//! owner's `c:`/`si:` id (or uuid) — a drive something was shared from, or the
//! drive of a Silicon the caller looks after. [`Client::drives`] lists the
//! drives the caller opens with the owner's rights, [`Client::shared`] what
//! others shared with it, and [`Client::entry_at`] resolves an address
//! (`/c:ada/notes/plan.md`).
//!
//! # Reading the answers
//!
//! Briefcase reports an entry the caller may not read exactly as one that does
//! not exist, so [`Error::is_not_found`] covers both and nothing confirms that
//! a hidden entry is there. [`Error::code`] carries the stable error code,
//! [`Error::hint`] what to do next when Briefcase said, and
//! [`Error::retry_after`] the wait a spent upload allowance names.
//!
//! # Authentication
//!
//! Briefcase authenticates with Silicon Accounts. A request carries the
//! account's access token issued to Briefcase (`aud = briefcase`) as a bearer.
//! Briefcase's own tools sign accounts in with the `accounts` feature
//! ([`accounts::SignIn`]: the device flow for Carbons, a short-lived token from
//! `silicon-accounts login --app briefcase -q` for Silicons); other apps sign
//! accounts in to themselves and act here with the [`delegated`] operations,
//! each carrying a Silicon Accounts User verification proof
//! (`Authorization: Proof sap_…`) instead of a bearer.
//!
//! [Silicon Briefcase]: https://briefcase.teamofsilicons.com

#[cfg(feature = "accounts")]
pub mod accounts;
mod api;
mod client;
mod config;
mod contract;
mod error;
mod media;
pub mod models;
mod requests;

pub use api::delegated;
pub use api::delegated::{
    DelegatedCancelUpload, DelegatedCommitUpload, DelegatedCreateFolder, DelegatedInvite,
    DelegatedLinkAccess, DelegatedListEntries, DelegatedManifest, DelegatedOperation,
    DelegatedReadFile, DelegatedReserveUpload, DelegatedTrashEntry, DelegatedUploadQuery,
    DelegatedUploadReservation, DelegatedUploadState, DelegatedUploadStatus, Proof,
    UploadCapability,
};
pub use api::{
    BugReport, ContentStream, ExpiryChange, Invitation, InvitationPage, Invite, LinkAccess,
    LogEvent, LogPage, PublicEntry, PublicPage, Recipient, ReportReceipt,
};
pub use client::{Client, IdempotencyKey};
pub use config::{
    Config, Credential, DEFAULT_API_URL, DEFAULT_CONNECT_TIMEOUT, DEFAULT_REQUEST_TIMEOUT,
    DEFAULT_TRANSFER_TIMEOUT,
};
pub use contract::{
    API_VERSION, CONTRACT_VERSION, OPERATIONS, OperationRevision, ServedOperation, ServiceVersion,
};
pub use error::{ApiError, Error, IncompatibleContract, OperationMismatch, Result};
pub use media::{DEFAULT_CONTENT_TYPE, guess_content_type};
pub use models::{
    AccessRight, Account, AccountKind, AccountRef, AccountSummary, AccountsInfo, ActivityEvent,
    ActorRef, ActorType, AllowedAccount, AllowedAccounts, ArchiveItem, ArchiveItemType,
    ArchiveListing, BucketConfiguration, BucketConfigurationState, BucketConfigurationStatus,
    DailyUsageMeasure, Drive, DriveList, DriveRelation, DriveUsage, EffectiveAccess,
    EffectivePermission, EncryptionMode, Entry, EntryPage, EntryType, EntryVisibility, FileVersion,
    FileVersionPage, Moved, Notification, NotificationDecision, NotificationInbox,
    NotificationKind, NotificationSubject, PermissionGrant, PermissionInspection, RenderKind,
    SearchResult, ServiceStatus, SharedEntry, SharedPage, UsageMeasure,
};
pub use requests::{
    ByteRange, Destination, EntryUpdate, ListEntries, NewFolder, NewGrant, PermissionQuery, Upload,
    UploadSource,
};

/// Operational telemetry and explicit opt-out controls.
pub mod telemetry;
