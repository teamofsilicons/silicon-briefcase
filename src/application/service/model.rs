//! Commands, repository snapshots, and safe service views.

use time::OffsetDateTime;

use crate::{
    application::idempotency::IdempotencyKey,
    domain::{
        access::{AccessDecision, AccessRequestStatus},
        actor::{ActorRef, ApplicationId, OrganizationId, RequestAuthContext},
        entry::{EntryBoundary, EntryKind, EntryName, EntryPath, RootType, SystemEntryKind},
        ids::{AccessRequestId, EntryId, GrantId, VersionId},
        permission::{
            AccessLevel, EffectiveAccess, EffectiveAuthorization, EffectiveAuthorizationInput,
            EntryVisibility, GrantApplication, PermissionGrant,
        },
        version::{VersionNumber, VersionSource},
    },
};

use super::{MetadataServiceError, ValidationError};

/// Maximum page size for entry, permission, version, and bin listing.
pub const MAX_PAGE_SIZE: u16 = 100;
/// Default page size from the `OpenAPI` contract.
pub const DEFAULT_PAGE_SIZE: u16 = 50;
/// Maximum search result count.
pub const MAX_SEARCH_RESULTS: u8 = 20;

/// Idempotency material shared by every externally initiated mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationMetadata {
    /// Validated client key; optional only where `OpenAPI` has not declared it yet.
    pub idempotency_key: Option<IdempotencyKey>,
    /// SHA-256 of the operation name and canonical request representation.
    pub request_fingerprint: [u8; 32],
}

impl MutationMetadata {
    /// Constructs mutation metadata after canonical request hashing.
    #[must_use]
    pub const fn new(
        idempotency_key: Option<IdempotencyKey>,
        request_fingerprint: [u8; 32],
    ) -> Self {
        Self {
            idempotency_key,
            request_fingerprint,
        }
    }

    pub(super) fn require_key(&self) -> Result<(), ValidationError> {
        if self.idempotency_key.is_some() {
            Ok(())
        } else {
            Err(ValidationError::new(
                "idempotency_key",
                "is required for this operation",
            ))
        }
    }
}

/// Validated pagination request with an opaque repository cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    /// Cursor returned by the preceding page.
    pub cursor: Option<String>,
    /// Maximum number of returned items.
    pub limit: u16,
}

impl PageRequest {
    /// Constructs a bounded page request.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] when `limit` is outside `1..=100` or a
    /// supplied cursor is empty.
    pub fn new(cursor: Option<String>, limit: u16) -> Result<Self, ValidationError> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(ValidationError::new("limit", "must be between 1 and 100"));
        }
        if cursor.as_ref().is_some_and(String::is_empty) {
            return Err(ValidationError::new("cursor", "must not be empty"));
        }
        Ok(Self { cursor, limit })
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            cursor: None,
            limit: DEFAULT_PAGE_SIZE,
        }
    }
}

/// A cursor-paginated result.
#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    /// Items in stable repository order.
    pub items: Vec<T>,
    /// Cursor for the next page, when more items exist.
    pub next_cursor: Option<String>,
}

/// Complete active or recoverable entry metadata used by the application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntryView {
    /// Entry identifier.
    pub id: EntryId,
    /// Organization security boundary.
    pub organization_id: OrganizationId,
    /// File or folder kind.
    pub kind: EntryKind,
    /// Validated display name.
    pub name: EntryName,
    /// Materialized organization-relative path used by the permanent URL.
    pub path: EntryPath,
    /// Parent folder, or organization root.
    pub parent_id: Option<EntryId>,
    /// Inherited access boundary.
    pub boundary: EntryBoundary,
    /// Entry owner.
    pub owner: ActorRef,
    /// Server-verified originating OBO application.
    pub origin_application_id: Option<ApplicationId>,
    /// File media type.
    pub content_type: Option<String>,
    /// Current file size.
    pub size: Option<u64>,
    /// Current immutable content version.
    pub current_version_id: Option<VersionId>,
    /// Creation time.
    pub created_at: OffsetDateTime,
    /// Last metadata change.
    pub updated_at: OffsetDateTime,
    /// Recoverable deletion time.
    pub deleted_at: Option<OffsetDateTime>,
}

/// Whether a resolved grant is direct or inherited from an ancestor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedGrantScope {
    /// Created on the target entry.
    Direct,
    /// Created on a proven ancestor.
    Inherited,
}

/// An explicit grant resolved by the repository for policy evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPermissionGrant {
    /// Grant domain entity.
    pub grant: PermissionGrant,
    /// Relationship to the target entry.
    pub scope: ResolvedGrantScope,
}

/// Entry metadata plus every trusted input needed by domain authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizableEntry {
    /// Entry metadata.
    pub entry: EntryView,
    /// Reserved reconciled-system classification.
    pub system_kind: Option<SystemEntryKind>,
    /// Relevant direct and inheritable ancestor grants.
    pub grants: Vec<ResolvedPermissionGrant>,
    /// Whether a visible descendant requires this folder for navigation.
    pub required_for_traversal: bool,
}

impl AuthorizableEntry {
    /// Evaluates shared domain policy against this repository snapshot.
    #[must_use]
    pub fn authorization(&self, context: &RequestAuthContext) -> EffectiveAuthorization {
        let grants: Vec<_> = self
            .grants
            .iter()
            .map(|resolved| match resolved.scope {
                ResolvedGrantScope::Direct => GrantApplication::Direct(&resolved.grant),
                ResolvedGrantScope::Inherited => GrantApplication::Inherited(&resolved.grant),
            })
            .collect();
        crate::domain::permission::evaluate_authorization(&EffectiveAuthorizationInput {
            context,
            entry_organization_id: &self.entry.organization_id,
            entry_id: self.entry.id,
            entry_kind: self.entry.kind,
            system_kind: self.system_kind,
            boundary: &self.entry.boundary,
            owner: &self.entry.owner,
            origin_application_id: self.entry.origin_application_id.as_ref(),
            grants: &grants,
            required_for_traversal: self.required_for_traversal,
        })
    }
}

/// Full entry metadata paired with stable v1 effective access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedEntryView {
    /// Complete entry metadata.
    pub entry: EntryView,
    /// API-facing effective access labels.
    pub effective_access: Vec<EffectiveAccess>,
}

/// Minimal metadata safe for traversing to a visible descendant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraversalEntryView {
    /// Folder identifier.
    pub id: EntryId,
    /// Parent folder.
    pub parent_id: Option<EntryId>,
    /// Folder display name.
    pub name: EntryName,
    /// Inherited API boundary discriminator.
    pub root_type: RootType,
}

/// A normal visible entry or a structurally redacted traversal ancestor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EntryListItem {
    /// Caller has normal read visibility.
    Full(Box<AuthorizedEntryView>),
    /// Caller may navigate through the folder but not inspect its contents.
    Traversal(TraversalEntryView),
}

impl AuthorizableEntry {
    pub(super) fn into_list_item(
        self,
        authorization: EffectiveAuthorization,
    ) -> Option<EntryListItem> {
        match authorization.visibility() {
            EntryVisibility::Traversal if self.entry.kind == EntryKind::Folder => {
                Some(EntryListItem::Traversal(TraversalEntryView {
                    id: self.entry.id,
                    parent_id: self.entry.parent_id,
                    name: self.entry.name,
                    root_type: self.entry.boundary.root_type(),
                }))
            }
            EntryVisibility::Hidden | EntryVisibility::Traversal => None,
            EntryVisibility::Full => Some(EntryListItem::Full(
                AuthorizedEntryView {
                    entry: self.entry,
                    effective_access: authorization.capabilities().effective_access(),
                }
                .into(),
            )),
        }
    }

    pub(super) fn into_full_view(
        self,
        authorization: EffectiveAuthorization,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        if authorization.visibility() != EntryVisibility::Full {
            return Err(MetadataServiceError::NotFound);
        }
        Ok(AuthorizedEntryView {
            entry: self.entry,
            effective_access: authorization.capabilities().effective_access(),
        })
    }
}

/// Query for visible children of an organization root or folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListEntriesQuery {
    /// Parent folder; omission means organization root.
    pub parent_id: Option<EntryId>,
    /// Cursor pagination.
    pub page: PageRequest,
}

/// Initial grant attached atomically to a newly created folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitialPermission {
    /// Current organization member.
    pub principal: ActorRef,
    /// Read or write access.
    pub access: AccessLevel,
    /// Whether access flows to descendants.
    pub inherits_to_descendants: bool,
}

/// Validated folder-creation command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateFolderCommand {
    /// Folder name.
    pub name: EntryName,
    /// Parent folder; omission creates a user root.
    pub parent_id: Option<EntryId>,
    /// Required only for user-root creation and inherited otherwise.
    pub root_boundary: Option<EntryBoundary>,
    /// Grants created in the same transaction.
    pub invitees: Vec<InitialPermission>,
}

impl CreateFolderCommand {
    /// Validates root-boundary presence against parent semantics.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] unless a root boundary is present exactly
    /// when `parent_id` is absent.
    pub fn new(
        name: EntryName,
        parent_id: Option<EntryId>,
        root_boundary: Option<EntryBoundary>,
        invitees: Vec<InitialPermission>,
    ) -> Result<Self, ValidationError> {
        if parent_id.is_none() != root_boundary.is_some() {
            return Err(ValidationError::new(
                "root_boundary",
                "is required exactly when parent_id is omitted",
            ));
        }
        Ok(Self {
            name,
            parent_id,
            root_boundary,
            invitees,
        })
    }
}

/// Repository-ready folder creation with policy-derived fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateFolderMutation {
    /// Server-generated entry identifier.
    pub entry_id: EntryId,
    /// Validated command.
    pub command: CreateFolderCommand,
    /// Inherited or root-declared boundary.
    pub boundary: EntryBoundary,
    /// Represented owner.
    pub owner: ActorRef,
    /// Verified OBO application origin.
    pub origin_application_id: Option<ApplicationId>,
}

/// Rename and/or move an existing entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateEntryCommand {
    /// Target entry.
    pub entry_id: EntryId,
    /// New name, when renaming.
    pub name: Option<EntryName>,
    /// New parent, when moving. The v1 contract cannot move to organization root.
    pub parent_id: Option<EntryId>,
}

impl UpdateEntryCommand {
    /// Rejects an empty patch.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] when neither a new name nor parent is
    /// supplied.
    pub fn new(
        entry_id: EntryId,
        name: Option<EntryName>,
        parent_id: Option<EntryId>,
    ) -> Result<Self, ValidationError> {
        if name.is_none() && parent_id.is_none() {
            return Err(ValidationError::new(
                "entry_patch",
                "must include name or parent_id",
            ));
        }
        Ok(Self {
            entry_id,
            name,
            parent_id,
        })
    }
}

/// Command to create an explicit grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantPermissionCommand {
    /// Target entry.
    pub entry_id: EntryId,
    /// Current organization member.
    pub principal: ActorRef,
    /// Access level.
    pub access: AccessLevel,
    /// Whether access flows to descendants.
    pub inherits_to_descendants: bool,
}

/// Command to revoke one explicit grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevokePermissionCommand {
    /// Target entry.
    pub entry_id: EntryId,
    /// Grant to revoke.
    pub grant_id: GrantId,
}

/// Command to request access without revealing hidden metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAccessCommand {
    /// Target entry from a permanent URL.
    pub entry_id: EntryId,
    /// Requested access level.
    pub access: AccessLevel,
    /// Optional user-supplied reason.
    pub reason: Option<String>,
}

impl RequestAccessCommand {
    /// Validates the documented 1,000-character reason limit.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] when the optional reason contains more than
    /// 1,000 Unicode scalar values after trimming.
    pub fn new(
        entry_id: EntryId,
        access: AccessLevel,
        reason: Option<String>,
    ) -> Result<Self, ValidationError> {
        let reason = reason.map(|value| value.trim().to_owned());
        if reason
            .as_ref()
            .is_some_and(|value| value.chars().count() > 1_000)
        {
            return Err(ValidationError::new(
                "reason",
                "must contain at most 1000 characters",
            ));
        }
        Ok(Self {
            entry_id,
            access,
            reason: reason.filter(|value| !value.is_empty()),
        })
    }
}

/// Persisted access-request view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessRequestView {
    /// Request identifier.
    pub id: AccessRequestId,
    /// Requested entry.
    pub entry_id: EntryId,
    /// Requesting member.
    pub requested_by: ActorRef,
    /// Requested access.
    pub requested_access: AccessLevel,
    /// Optional reason.
    pub reason: Option<String>,
    /// Current state.
    pub status: AccessRequestStatus,
    /// Access actually granted.
    pub granted_access: Option<AccessLevel>,
    /// Decision actor.
    pub decided_by: Option<ActorRef>,
    /// Decision time.
    pub decided_at: Option<OffsetDateTime>,
    /// Grant created by approval.
    pub permission_grant_id: Option<GrantId>,
    /// Creation time.
    pub created_at: OffsetDateTime,
    /// Last state change.
    pub updated_at: OffsetDateTime,
}

/// Access request paired with the requested entry's policy snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizableAccessRequest {
    /// Request data.
    pub request: AccessRequestView,
    /// Requested entry authorization data.
    pub entry: AuthorizableEntry,
}

/// Command to approve or deny an access request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecideAccessRequestCommand {
    /// Pending request.
    pub request_id: AccessRequestId,
    /// Terminal decision.
    pub decision: AccessDecision,
}

/// Permission page query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListPermissionsQuery {
    /// Target entry.
    pub entry_id: EntryId,
    /// Cursor pagination.
    pub page: PageRequest,
}

/// Validated permission-filtered search query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchQuery {
    /// Non-empty user query.
    pub query: String,
    /// Maximum result count from 1 through 20.
    pub limit: u8,
}

impl SearchQuery {
    /// Trims and validates a search query.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] for an empty query or a limit outside
    /// `1..=20`.
    pub fn new(query: impl Into<String>, limit: u8) -> Result<Self, ValidationError> {
        let query = query.into().trim().to_owned();
        if query.is_empty() {
            return Err(ValidationError::new("q", "must not be empty"));
        }
        if !(1..=MAX_SEARCH_RESULTS).contains(&limit) {
            return Err(ValidationError::new("limit", "must be between 1 and 20"));
        }
        Ok(Self { query, limit })
    }
}

/// Repository search candidate with authorization facts.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchCandidate {
    /// Candidate entry and policy inputs.
    pub entry: AuthorizableEntry,
    /// Repository ranking score.
    pub score: f64,
    /// Whether the filename matched.
    pub filename_match: bool,
    /// Count of extracted-content hits.
    pub content_hits: u32,
    /// Redacted content snippets.
    pub snippets: Vec<String>,
}

/// Safe visible search result.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResultView {
    /// Authorized entry metadata.
    pub entry: AuthorizedEntryView,
    /// Stable ranking score.
    pub score: f64,
    /// Whether the filename matched.
    pub filename_match: bool,
    /// Extracted-content hit count.
    pub content_hits: u32,
    /// Optional snippets safe for the authorized actor.
    pub snippets: Vec<String>,
}

/// Immutable file-version metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileVersionView {
    /// Version identifier.
    pub id: VersionId,
    /// Monotonic one-based number.
    pub number: VersionNumber,
    /// Exact byte size.
    pub size: u64,
    /// Creating actor.
    pub created_by: ActorRef,
    /// Creation provenance.
    pub source: VersionSource,
    /// Creation time.
    pub created_at: OffsetDateTime,
}

/// Query for retained versions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListVersionsQuery {
    /// Current file entry.
    pub entry_id: EntryId,
    /// Cursor pagination, capped by the 50-version retention rule.
    pub page: PageRequest,
}

/// Query for recoverable entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListBinQuery {
    /// Cursor pagination.
    pub page: PageRequest,
}

/// Command to restore a recoverable entry subtree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestoreBinEntryCommand {
    /// Recoverable subtree root.
    pub entry_id: EntryId,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::domain::{
        entry::{EntryBoundary, EntryName},
        ids::EntryId,
        permission::AccessLevel,
    };

    use super::{CreateFolderCommand, PageRequest, RequestAccessCommand, SearchQuery};

    #[test]
    fn page_request_enforces_contract_bounds() {
        assert!(PageRequest::new(None, 0).is_err());
        assert!(PageRequest::new(None, 101).is_err());
        assert!(PageRequest::new(Some(String::new()), 50).is_err());
        assert!(PageRequest::new(Some("cursor".to_owned()), 100).is_ok());
    }

    #[test]
    fn folder_boundary_is_present_exactly_for_user_roots() -> Result<(), Box<dyn Error>> {
        let name = EntryName::new("Documents")?;
        assert!(CreateFolderCommand::new(name.clone(), None, None, Vec::new()).is_err());
        assert!(
            CreateFolderCommand::new(
                name.clone(),
                Some(EntryId::new()),
                Some(EntryBoundary::Private),
                Vec::new(),
            )
            .is_err()
        );
        assert!(
            CreateFolderCommand::new(name, None, Some(EntryBoundary::Private), Vec::new()).is_ok()
        );
        Ok(())
    }

    #[test]
    fn access_request_normalizes_and_limits_reason() -> Result<(), Box<dyn Error>> {
        let command =
            RequestAccessCommand::new(EntryId::new(), AccessLevel::Read, Some("  ".to_owned()))?;
        assert_eq!(command.reason, None);
        assert!(
            RequestAccessCommand::new(EntryId::new(), AccessLevel::Write, Some("x".repeat(1_001)),)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn search_query_is_trimmed_and_bounded() -> Result<(), Box<dyn Error>> {
        let query = SearchQuery::new("  quarterly report  ", 20)?;
        assert_eq!(query.query, "quarterly report");
        assert!(SearchQuery::new(" ", 1).is_err());
        assert!(SearchQuery::new("valid", 21).is_err());
        Ok(())
    }
}
