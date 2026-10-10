//! Delegated operations: another app acting for an account with a Silicon
//! Accounts User verification proof.
//!
//! An app that a Carbon or Silicon asked to work with its files (Waveform
//! storing speech, Extend storing session recordings…) obtains a proof from
//! Silicon Accounts (`POST /v1/proofs/user-verification`, receiving app
//! `briefcase`, the scopes below) and sends it as `Authorization: Proof sap_…`.
//! Briefcase verifies it with Silicon Accounts on every call, checks that the
//! issuing app is allowed the route's scope, and acts as the account the proof
//! speaks for; what it creates records the issuing app. Apps reach only
//! `apps/{issuing app}/` in that account's own drive, except the first-party
//! apps Briefcase's operator lists.
//!
//! Each route requires exactly one scope, its operation's [`DelegatedOperation::SCOPE`]
//! (`briefcase.folders.create`, `briefcase.uploads.reserve`…). Immutable
//! manifests keep logical idempotency (`operation_id`) and content integrity;
//! no account bearer is ever sent with a proof.

use std::{fmt, marker::PhantomData};

use reqwest::{Method, RequestBuilder, header::HeaderValue};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

mod uploads;
pub use uploads::{
    DelegatedCancelUpload, DelegatedCommitUpload, DelegatedReserveUpload, DelegatedUploadQuery,
    DelegatedUploadReservation, DelegatedUploadState, DelegatedUploadStatus, UploadCapability,
};

/// The routes another app may call with a proof, and the scope each requires.
pub const SCOPES: [(&str, &str); 10] = [
    ("briefcase.folders.create", CREATE_FOLDER_PATH),
    ("briefcase.entries.list", LIST_ENTRIES_PATH),
    ("briefcase.files.read", READ_FILE_PATH),
    ("briefcase.entries.trash", TRASH_ENTRY_PATH),
    ("briefcase.uploads.reserve", "/api/v1/obo/uploads/reserve"),
    ("briefcase.uploads.commit", "/api/v1/obo/uploads/commit"),
    ("briefcase.uploads.status", "/api/v1/obo/uploads/status"),
    ("briefcase.uploads.cancel", "/api/v1/obo/uploads/cancel"),
    ("briefcase.invitations.create", "/api/v1/obo/invitations"),
    ("briefcase.link_access.update", "/api/v1/obo/link-access"),
];

use crate::{ByteRange, Client, ContentStream, Entry, EntryPage, Error, Result, client::json_body};

/// HTTP method used for every delegated JSON manifest.
pub const METHOD: &str = "POST";
/// Proof scope for folder creation.
pub const CREATE_FOLDER_SCOPE: &str = "briefcase.folders.create";
/// Path of folder creation.
pub const CREATE_FOLDER_PATH: &str = "/api/v1/obo/folders/create";
/// Proof scope for entry listing.
pub const LIST_ENTRIES_SCOPE: &str = "briefcase.entries.list";
/// Path of entry listing.
pub const LIST_ENTRIES_PATH: &str = "/api/v1/obo/entries/list";
/// Proof scope for reading a file.
pub const READ_FILE_SCOPE: &str = "briefcase.files.read";
/// Path of reading a file.
pub const READ_FILE_PATH: &str = "/api/v1/obo/files/read";
/// Proof scope for moving an app-created entry to the bin.
pub const TRASH_ENTRY_SCOPE: &str = "briefcase.entries.trash";
/// Path of moving an app-created entry to the bin.
pub const TRASH_ENTRY_PATH: &str = "/api/v1/obo/entries/trash";

/// A Silicon Accounts User verification proof (`sap_…`) whose receiving app is
/// `briefcase`, sent as `Authorization: Proof sap_…`.
///
/// A proof may be reused for several calls while it is valid; refreshing it
/// (with its `sapr_…` refresh token) is the issuing app's job and happens at
/// Silicon Accounts, never here. `Debug` never prints it.
#[derive(Clone)]
pub struct Proof(SecretString);

impl Proof {
    /// Wraps a proof without interpreting it beyond its public prefix.
    ///
    /// # Errors
    ///
    /// Rejects anything that is not a `sap_…` proof — notably a proof refresh
    /// token (`sapr_…`), which must never leave the issuing app — and values
    /// that cannot be an HTTP header. The value is never repeated in the error.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let value = value.trim();
        if value.starts_with("sapr_") {
            return Err(Error::Configuration(
                "this is a proof refresh token (sapr_…), which stays with the app that holds it; pass the proof itself (sap_…)".into(),
            ));
        }
        if !value.starts_with("sap_") || value.len() <= "sap_".len() {
            return Err(Error::Configuration(
                "a Briefcase delegated call takes a Silicon Accounts User verification proof (sap_…) issued for receiving app `briefcase`".into(),
            ));
        }
        if HeaderValue::from_str(value).is_err() || value.contains(char::is_whitespace) {
            return Err(Error::Configuration(
                "the proof contains characters that cannot travel in a header".into(),
            ));
        }
        Ok(Self(SecretString::from(value.to_owned())))
    }

    fn into_header(self) -> Result<HeaderValue> {
        let mut header = HeaderValue::from_str(&format!("Proof {}", self.0.expose_secret()))
            .map_err(|_| Error::Configuration("invalid proof header".into()))?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl fmt::Debug for Proof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Proof(<redacted>)")
    }
}

mod sealed {
    pub trait Operation {
        fn validate(&self) -> crate::Result<()>;
    }
}

/// One of the fixed delegated JSON operations supported by this client.
///
/// Sealed to prevent a manifest for one route being sent to another.
pub trait DelegatedOperation: sealed::Operation + Serialize {
    /// The one proof scope the route requires (the issuing app requests it
    /// from Silicon Accounts).
    const SCOPE: &'static str;
    /// Exact, versioned request path.
    const PATH: &'static str;
}

/// Immutable exact bytes to hash for integrity and logical idempotency, then transmit.
///
/// Prepared manifests can be cloned or retained for logical retries; changing
/// the request value afterwards cannot change these bytes.
#[derive(Clone)]
pub struct DelegatedManifest<T: DelegatedOperation> {
    bytes: Vec<u8>,
    sha256: String,
    operation: PhantomData<fn() -> T>,
}

impl<T: DelegatedOperation> DelegatedManifest<T> {
    /// Validates and serializes the request exactly once.
    ///
    /// # Errors
    ///
    /// Returns a local configuration error for invalid identifiers, ambiguous
    /// list parents, invalid limits, or an unserializable request.
    pub fn new(request: &T) -> Result<Self> {
        request.validate()?;
        let bytes = json_body(request)?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok(Self {
            bytes,
            sha256,
            operation: PhantomData,
        })
    }

    /// The exact JSON bytes the SDK will send, without re-serialization.
    #[must_use]
    pub fn body_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Lowercase hexadecimal SHA-256 of `body_bytes()`, for local integrity and logical idempotency.
    #[must_use]
    pub fn body_sha256(&self) -> &str {
        &self.sha256
    }

    /// The HTTP method of the route.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        METHOD
    }

    /// The full versioned path of the route.
    #[must_use]
    pub const fn path(&self) -> &'static str {
        T::PATH
    }

    /// The proof scope the route requires.
    #[must_use]
    pub const fn scope(&self) -> &'static str {
        T::SCOPE
    }
}

impl<T: DelegatedOperation> fmt::Debug for DelegatedManifest<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegatedManifest")
            .field("scope", &T::SCOPE)
            .field("path", &T::PATH)
            .field("body_length", &self.bytes.len())
            .field("body_sha256", &self.sha256)
            .finish()
    }
}

/// Creates one child folder using a stable caller-owned logical operation UUID.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedCreateFolder {
    /// Non-nil UUID retained with this unchanged manifest across logical retries.
    pub operation_id: Uuid,
    /// Existing parent path; an empty string selects the app's own folder,
    /// `apps/{issuing app}/` in the account's drive.
    pub parent_path: String,
    /// A single child folder name, not a path.
    pub name: String,
}

/// Lists the ordinary privacy-filtered entry view, with every input in an immutable manifest.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedListEntries {
    /// Existing parent UUID, mutually exclusive with `path`.
    pub parent_id: Option<Uuid>,
    /// Existing parent path; with neither, the app's own folder is listed.
    pub path: Option<String>,
    /// First-party apps only: the drive to list or resolve `path` in, by its
    /// owner's `c:`/`si:` id or uuid (the account's own by default). Other apps
    /// are refused with `drive_not_delegated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
    /// Optional filter expression; without a parent, searches the visible tree.
    pub filter: Option<String>,
    /// Opaque cursor from the preceding page; each page is verified online.
    pub cursor: Option<String>,
    /// Page size from 1 through 100, defaulting to 100.
    pub limit: Option<u16>,
}

/// Reads a current file with the range and disposition bound into its JSON body.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedReadFile {
    /// Non-nil UUID of the exact file.
    pub entry_id: Uuid,
    /// Optional HTTP byte-range syntax, for example `bytes=0-1023`.
    pub range: Option<String>,
    /// `true` for attachment delivery; `false` for sandboxed inline content.
    #[serde(default)]
    pub download: bool,
}

impl DelegatedReadFile {
    /// Uses a typed byte range instead of constructing HTTP range syntax.
    #[must_use]
    pub fn with_range(mut self, range: ByteRange) -> Self {
        self.range = Some(range.header_value());
        self
    }
}

/// Moves an entry the issuing app created to the recoverable bin (never
/// anything else: `not_created_by_app`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedTrashEntry {
    /// Non-nil UUID retained with this unchanged manifest across logical retries.
    pub operation_id: Uuid,
    /// Non-nil UUID of the entry to trash.
    pub entry_id: Uuid,
}

macro_rules! delegated_operation {
    ($request:ty, $scope:ident, $path:ident) => {
        impl DelegatedOperation for $request {
            const SCOPE: &'static str = $scope;
            const PATH: &'static str = $path;
        }
        impl $request {
            /// Freezes the exact manifest bytes before transmitting the request.
            ///
            /// # Errors
            ///
            /// Returns a local error when validation or JSON serialization fails.
            pub fn prepare(&self) -> Result<DelegatedManifest<Self>> {
                DelegatedManifest::new(self)
            }
        }
    };
}

delegated_operation!(
    DelegatedCreateFolder,
    CREATE_FOLDER_SCOPE,
    CREATE_FOLDER_PATH
);
delegated_operation!(DelegatedListEntries, LIST_ENTRIES_SCOPE, LIST_ENTRIES_PATH);
delegated_operation!(DelegatedReadFile, READ_FILE_SCOPE, READ_FILE_PATH);
delegated_operation!(DelegatedTrashEntry, TRASH_ENTRY_SCOPE, TRASH_ENTRY_PATH);

fn non_nil(id: Uuid, field: &str) -> Result<()> {
    if id.is_nil() {
        Err(Error::Configuration(format!(
            "delegated {field} must not be nil"
        )))
    } else {
        Ok(())
    }
}

impl sealed::Operation for DelegatedCreateFolder {
    fn validate(&self) -> Result<()> {
        non_nil(self.operation_id, "operation_id")
    }
}

impl sealed::Operation for DelegatedListEntries {
    fn validate(&self) -> Result<()> {
        if self.parent_id.is_some() && self.path.is_some() {
            return Err(Error::Configuration(
                "delegated listing accepts parent_id or path, not both".into(),
            ));
        }
        if let Some(parent) = self.parent_id {
            non_nil(parent, "parent_id")?;
        }
        if self
            .drive
            .as_deref()
            .is_some_and(|drive| drive.trim().is_empty())
        {
            return Err(Error::Configuration(
                "delegated listing drive must name an account when it is given".into(),
            ));
        }
        if self.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
            return Err(Error::Configuration(
                "delegated listing limit must be from 1 through 100".into(),
            ));
        }
        Ok(())
    }
}

impl sealed::Operation for DelegatedReadFile {
    fn validate(&self) -> Result<()> {
        non_nil(self.entry_id, "entry_id")?;
        if self
            .range
            .as_ref()
            .is_some_and(|range| HeaderValue::from_str(range).is_err())
        {
            return Err(Error::Configuration(
                "invalid delegated range header".into(),
            ));
        }
        Ok(())
    }
}

impl sealed::Operation for DelegatedTrashEntry {
    fn validate(&self) -> Result<()> {
        non_nil(self.operation_id, "operation_id")?;
        non_nil(self.entry_id, "entry_id")
    }
}

impl Client {
    /// Creates a folder for the account the proof speaks for.
    ///
    /// # Errors
    ///
    /// Returns proof, authorization, conflict, or transport errors without retry.
    pub async fn create_folder_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedCreateFolder>,
    ) -> Result<Entry> {
        let request = self.delegated_request(proof, manifest)?;
        self.receive_json(request.timeout(self.request_timeout()))
            .await
    }

    /// Lists one page for the account the proof speaks for.
    ///
    /// # Errors
    ///
    /// Returns proof, visibility, pagination, or transport errors without retry.
    pub async fn list_entries_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedListEntries>,
    ) -> Result<EntryPage> {
        let request = self.delegated_request(proof, manifest)?;
        self.receive_json(request.timeout(self.request_timeout()))
            .await
    }

    /// Streams a file with no unbound Range header or disposition query.
    ///
    /// # Errors
    ///
    /// Returns proof, visibility, range, or transport errors without retry.
    /// Reading or dropping the returned stream never triggers maintenance.
    pub async fn read_file_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedReadFile>,
    ) -> Result<ContentStream> {
        let request = self.delegated_request(proof, manifest)?;
        let response = self
            .receive(request.timeout(self.transfer_timeout()))
            .await?;
        Ok(ContentStream::new(response))
    }

    /// Moves an entry the issuing app created to the bin, with a stable
    /// operation UUID.
    ///
    /// # Errors
    ///
    /// Returns proof, subtree-delete authorization, or transport errors without retry.
    pub async fn trash_entry_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedTrashEntry>,
    ) -> Result<()> {
        let request = self.delegated_request(proof, manifest)?;
        self.receive(request.timeout(self.request_timeout()))
            .await
            .map(drop)
    }

    fn delegated_request<T: DelegatedOperation>(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<T>,
    ) -> Result<RequestBuilder> {
        let segments: Vec<_> = manifest.path().trim_start_matches('/').split('/').collect();
        let url = self.origin_url(&segments)?;
        // A delegated request never also carries the configured account
        // bearer: the proof is its only credential.
        Ok(self
            .anonymous_request(Method::POST, url)
            .header(reqwest::header::AUTHORIZATION, proof.into_header()?)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(manifest.bytes.clone()))
    }
}

/// Shares an entry the app can reach with an account (scope
/// `briefcase.invitations.create`). The issuing app obtains the account's
/// agreement in its own flow before asking for this proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedInvite {
    /// Stable UUID reused on retry.
    pub operation_id: Uuid,
    /// The entry to share.
    pub entry_id: Uuid,
    /// Recipient (a `c:`/`si:` id) and granted rights.
    pub invitation: crate::Invite,
}
impl sealed::Operation for DelegatedInvite {
    fn validate(&self) -> Result<()> {
        non_nil(self.operation_id, "operation_id")?;
        non_nil(self.entry_id, "entry_id")
    }
}
impl DelegatedOperation for DelegatedInvite {
    const SCOPE: &'static str = "briefcase.invitations.create";
    const PATH: &'static str = "/api/v1/obo/invitations";
}
/// Turns anyone-with-the-link access on or off (scope
/// `briefcase.link_access.update`). The issuing app obtains the account's
/// agreement in its own flow before asking for this proof.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegatedLinkAccess {
    /// Stable logical operation ID.
    pub operation_id: Uuid,
    /// The entry.
    pub entry_id: Uuid,
    /// Desired explicit link setting.
    pub enabled: bool,
    /// With `enabled`, makes this an expiring link that ends after 1 to 43,200
    /// minutes. Part of the immutable request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_minutes: Option<u32>,
}
impl sealed::Operation for DelegatedLinkAccess {
    fn validate(&self) -> Result<()> {
        non_nil(self.operation_id, "operation_id")?;
        non_nil(self.entry_id, "entry_id")
    }
}
impl DelegatedOperation for DelegatedLinkAccess {
    const SCOPE: &'static str = "briefcase.link_access.update";
    const PATH: &'static str = "/api/v1/obo/link-access";
}
impl Client {
    /// Shares an entry for the account the proof speaks for.
    /// # Errors
    /// Returns proof, permission, recipient, or transport errors without retrying.
    pub async fn invite_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedInvite>,
    ) -> Result<crate::Invitation> {
        self.receive_json(
            self.delegated_request(proof, manifest)?
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Sets anyone-with-the-link access for the account the proof speaks for.
    /// # Errors
    /// Returns proof, permission, protected-folder, or transport errors without retrying.
    pub async fn set_link_access_on_behalf_of(
        &self,
        proof: Proof,
        manifest: &DelegatedManifest<DelegatedLinkAccess>,
    ) -> Result<crate::LinkAccess> {
        self.receive_json(
            self.delegated_request(proof, manifest)?
                .timeout(self.request_timeout()),
        )
        .await
    }
}
