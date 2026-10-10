//! Additive entry authorization and explicit permission grants.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use super::{
    actor::{ActorRef, ApplicationId, OrganizationId, RequestAuthContext},
    entry::{EntryKind, SystemEntryKind},
    ids::{EntryId, GrantId},
};

/// One right that a permission grant can convey.
///
/// The rights are independent, exactly as the product contract requires:
/// update authority never implies deletion, and write authority never implies
/// update. Read is the only right every grant carries, because nobody can act
/// on an entry they may not see.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRight {
    /// View and download the entry.
    Read,
    /// Add new content by creating children in a folder.
    Write,
    /// Change what already exists: rename, move, or replace content.
    Update,
    /// Move the entry to the recoverable bin.
    Delete,
}

/// Every right in a stable order.
pub const ALL_ACCESS_RIGHTS: [AccessRight; 4] = [
    AccessRight::Read,
    AccessRight::Write,
    AccessRight::Update,
    AccessRight::Delete,
];

impl AccessRight {
    /// Returns the effective-access label that already satisfies this right.
    ///
    /// The labels are entry-kind aware, so comparing against them is how a
    /// service decides whether a request would add anything at all.
    #[must_use]
    pub const fn satisfied_by(self) -> EffectiveAccess {
        match self {
            Self::Read => EffectiveAccess::Read,
            Self::Write => EffectiveAccess::Write,
            Self::Update => EffectiveAccess::Update,
            Self::Delete => EffectiveAccess::Delete,
        }
    }

    const fn mask(self) -> u8 {
        match self {
            Self::Read => 1 << 0,
            Self::Write => 1 << 1,
            Self::Update => 1 << 2,
            Self::Delete => 1 << 3,
        }
    }
}

/// A non-empty set of rights conveyed by one grant or request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GrantedAccess(u8);

impl GrantedAccess {
    /// Read-only access.
    pub const READ_ONLY: Self = Self(AccessRight::Read.mask());

    /// Builds an access set from requested rights, always including read.
    #[must_use]
    pub fn new(rights: impl IntoIterator<Item = AccessRight>) -> Self {
        let bits = rights
            .into_iter()
            .fold(AccessRight::Read.mask(), |bits, right| bits | right.mask());
        Self(bits)
    }

    /// Rehydrates an access set from its persisted bit representation.
    ///
    /// # Errors
    ///
    /// Returns [`GrantedAccessError`] when the value has unknown bits or does
    /// not include read.
    pub const fn from_bits(bits: u8) -> Result<Self, GrantedAccessError> {
        let known = AccessRight::Read.mask()
            | AccessRight::Write.mask()
            | AccessRight::Update.mask()
            | AccessRight::Delete.mask();
        if bits & !known != 0 || bits & AccessRight::Read.mask() == 0 {
            return Err(GrantedAccessError);
        }
        Ok(Self(bits))
    }

    /// Returns the persisted bit representation.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns whether the set conveys a right.
    #[must_use]
    pub const fn contains(self, right: AccessRight) -> bool {
        self.0 & right.mask() != 0
    }

    /// Returns whether the set conveys nothing beyond read.
    #[must_use]
    pub const fn is_read_only(self) -> bool {
        self.0 == AccessRight::Read.mask()
    }

    /// Iterates over conveyed rights in a stable order.
    pub fn rights(self) -> impl Iterator<Item = AccessRight> {
        ALL_ACCESS_RIGHTS
            .into_iter()
            .filter(move |right| self.contains(*right))
    }
}

/// An invalid persisted or transported access set.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("granted access must include read and only known rights")]
pub struct GrantedAccessError;

/// Whether an explicit grant applies only to its entry or also descendants.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionInheritance {
    /// The grant applies only to its source entry.
    EntryOnly,
    /// The grant applies to its source entry and all descendants.
    Descendants,
}

impl PermissionInheritance {
    /// Constructs the domain value used by the v1 `inherit` boolean.
    #[must_use]
    pub const fn from_inherit_flag(inherit: bool) -> Self {
        if inherit {
            Self::Descendants
        } else {
            Self::EntryOnly
        }
    }

    /// Returns the v1 `inherit` representation.
    #[must_use]
    pub const fn inherit_flag(self) -> bool {
        matches!(self, Self::Descendants)
    }
}

/// Complete persisted facts for an explicit permission grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionGrantParts {
    /// Unique grant identifier.
    pub id: GrantId,
    /// The drive holding the entry.
    pub organization_id: OrganizationId,
    /// Entry on which the grant was created.
    pub entry_id: EntryId,
    /// The account receiving the grant.
    pub principal: ActorRef,
    /// Rights conveyed by the grant.
    pub access: GrantedAccess,
    /// Whether the grant flows to descendants.
    pub inheritance: PermissionInheritance,
    /// Actor who created the grant.
    pub granted_by: ActorRef,
    /// UTC creation time.
    pub created_at: OffsetDateTime,
}

/// An explicit, additive access grant for a Carbon or Silicon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionGrant {
    parts: PermissionGrantParts,
    expires_at: Option<OffsetDateTime>,
}

impl PermissionGrant {
    /// Rehydrates or creates a permanent grant from already validated facts.
    #[must_use]
    pub const fn from_parts(parts: PermissionGrantParts) -> Self {
        Self {
            parts,
            expires_at: None,
        }
    }

    /// Marks the grant as an expiring share that ends at `expires_at`.
    ///
    /// `None` keeps it permanent. Expired shares are never loaded as grants,
    /// so a grant with an expiry is live until that instant.
    #[must_use]
    pub const fn with_expiry(mut self, expires_at: Option<OffsetDateTime>) -> Self {
        self.expires_at = expires_at;
        self
    }

    /// Returns when an expiring share ends, or `None` for a permanent grant.
    #[must_use]
    pub const fn expires_at(&self) -> Option<OffsetDateTime> {
        self.expires_at
    }

    /// Returns the grant identifier.
    #[must_use]
    pub const fn id(&self) -> GrantId {
        self.parts.id
    }

    /// Returns the drive holding the entry.
    #[must_use]
    pub const fn organization_id(&self) -> &OrganizationId {
        &self.parts.organization_id
    }

    /// Returns the entry on which the grant was created.
    #[must_use]
    pub const fn entry_id(&self) -> EntryId {
        self.parts.entry_id
    }

    /// Returns the receiving actor.
    #[must_use]
    pub const fn principal(&self) -> &ActorRef {
        &self.parts.principal
    }

    /// Returns the rights conveyed by the grant.
    #[must_use]
    pub const fn access(&self) -> GrantedAccess {
        self.parts.access
    }

    /// Returns the grant's inheritance behavior.
    #[must_use]
    pub const fn inheritance(&self) -> PermissionInheritance {
        self.parts.inheritance
    }

    /// Returns the actor who created the grant.
    #[must_use]
    pub const fn granted_by(&self) -> &ActorRef {
        &self.parts.granted_by
    }

    /// Returns the UTC creation time.
    #[must_use]
    pub const fn created_at(&self) -> OffsetDateTime {
        self.parts.created_at
    }
}

/// How a repository-resolved grant relates to the entry being authorized.
#[derive(Clone, Copy, Debug)]
pub enum GrantApplication<'a> {
    /// The grant was created directly on the target entry.
    Direct(&'a PermissionGrant),
    /// The grant was created on an ancestor of the target entry.
    Inherited(&'a PermissionGrant),
}

impl<'a> GrantApplication<'a> {
    fn grant(self) -> &'a PermissionGrant {
        match self {
            Self::Direct(grant) | Self::Inherited(grant) => grant,
        }
    }

    fn applies_to(self, target_entry_id: EntryId) -> bool {
        match self {
            Self::Direct(grant) => grant.entry_id() == target_entry_id,
            Self::Inherited(grant) => {
                grant.entry_id() != target_entry_id
                    && grant.inheritance() == PermissionInheritance::Descendants
            }
        }
    }
}

/// A fine-grained operation checked by domain policy.
///
/// The public API exposes `read`, `write`, `update`, `delete`, and
/// `manage_permissions`. Separating child creation and existing-entry mutation
/// prevents Public upload rights from broadening into rename or delete rights.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Capability {
    /// Read file content and complete metadata.
    Read,
    /// Create a direct child of a folder.
    CreateChild,
    /// Rename or move an existing entry.
    UpdateMetadata,
    /// Replace or restore the content of a file that already exists.
    WriteContent,
    /// Move an entry to the recoverable bin.
    Delete,
    /// Create and revoke explicit grants and manage link access.
    ManagePermissions,
}

impl Capability {
    const fn mask(self) -> u8 {
        match self {
            Self::Read => 1 << 0,
            Self::CreateChild => 1 << 1,
            Self::UpdateMetadata => 1 << 2,
            Self::WriteContent => 1 << 3,
            Self::Delete => 1 << 4,
            Self::ManagePermissions => 1 << 5,
        }
    }
}

const ALL_CAPABILITIES: [Capability; 6] = [
    Capability::Read,
    Capability::CreateChild,
    Capability::UpdateMetadata,
    Capability::WriteContent,
    Capability::Delete,
    Capability::ManagePermissions,
];

/// A compact set of effective capabilities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    /// Returns an empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns the complete internal capability set.
    #[must_use]
    pub const fn all() -> Self {
        Self((1 << ALL_CAPABILITIES.len()) - 1)
    }

    /// Returns the capabilities conveyed by an explicit access set.
    ///
    /// Write adds content that does not exist yet, update changes content that
    /// does, and delete stands alone. A grant never conveys permission
    /// management: only creating the entry and owning (or looking after) the
    /// drive do.
    #[must_use]
    pub fn from_granted_access(access: GrantedAccess) -> Self {
        let mut capabilities = Self(Capability::Read.mask());
        if access.contains(AccessRight::Write) {
            // Write adds content that does not exist yet, so it conveys child
            // creation and nothing more. Replacing the bytes of a file that
            // already exists is an update, and a grant that does not say
            // `update` never permits it.
            capabilities.insert(Capability::CreateChild);
        }
        if access.contains(AccessRight::Update) {
            capabilities.insert(Capability::UpdateMetadata);
            capabilities.insert(Capability::WriteContent);
        }

        capabilities
    }

    /// Returns whether the set includes a capability.
    #[must_use]
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.mask() != 0
    }

    /// Adds a capability.
    pub fn insert(&mut self, capability: Capability) {
        self.0 |= capability.mask();
    }

    /// Removes a capability.
    pub fn remove(&mut self, capability: Capability) {
        self.0 &= !capability.mask();
    }

    /// Adds every capability in another set.
    pub fn union_with(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Returns whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Iterates over capabilities in a stable order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        ALL_CAPABILITIES
            .into_iter()
            .filter(move |capability| self.contains(*capability))
    }

    /// Maps fine-grained policy to the API's effective-access vocabulary.
    ///
    /// The labels answer the product question "what can I do here?", so they
    /// distinguish adding new content from changing existing content and keep
    /// deletion separate from both.
    #[must_use]
    pub fn effective_access(self) -> Vec<EffectiveAccess> {
        let mut access = Vec::with_capacity(5);
        if self.contains(Capability::Read) {
            access.push(EffectiveAccess::Read);
        }
        if self.contains(Capability::CreateChild) {
            access.push(EffectiveAccess::Write);
        }
        if self.contains(Capability::UpdateMetadata) || self.contains(Capability::WriteContent) {
            access.push(EffectiveAccess::Update);
        }
        if self.contains(Capability::Delete) {
            access.push(EffectiveAccess::Delete);
        }
        if self.contains(Capability::ManagePermissions) {
            access.push(EffectiveAccess::ManagePermissions);
        }
        access
    }
}

/// The stable access labels returned in an entry response.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveAccess {
    /// Read and download access.
    Read,
    /// Adding content that does not exist yet.
    Write,
    /// Changing an entry that already exists.
    Update,
    /// Recoverable deletion.
    Delete,
    /// Explicit permission management.
    ManagePermissions,
}

/// How much entry metadata may be revealed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntryVisibility {
    /// Reveal nothing and use an opaque not-found response.
    Hidden,
    /// Reveal only the minimal folder metadata needed to reach a visible child.
    Traversal,
    /// Reveal normal metadata allowed by read access.
    Full,
}

/// All trusted facts needed to authorize one entry.
#[derive(Debug)]
pub struct EffectiveAuthorizationInput<'a> {
    /// The verified caller, the drive the request acts in, and the caller's
    /// relation to that drive.
    pub context: &'a RequestAuthContext,
    /// The drive that holds the entry.
    pub entry_organization_id: &'a OrganizationId,
    /// Entry being authorized.
    pub entry_id: EntryId,
    /// Whether the target is a file or folder.
    pub entry_kind: EntryKind,
    /// System classification, for structural folders.
    pub system_kind: Option<SystemEntryKind>,
    /// The account that created the entry.
    pub owner: &'a ActorRef,
    /// Server-derived application that originally created the entry.
    pub origin_application_id: Option<&'a ApplicationId>,
    /// Direct and repository-proven ancestor grants relevant to the entry.
    pub grants: &'a [GrantApplication<'a>],
    /// Whether the caller (or a Silicon it looks after) created a folder this
    /// entry sits inside.
    pub owns_ancestor: bool,
    /// Whether a visible descendant requires this otherwise-hidden folder.
    pub required_for_traversal: bool,
}

/// Domain authorization result for one entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectiveAuthorization {
    visibility: EntryVisibility,
    capabilities: CapabilitySet,
}

impl EffectiveAuthorization {
    /// Opaque denial for an entry outside the delegated application namespace.
    #[must_use]
    pub const fn hidden() -> Self {
        Self {
            visibility: EntryVisibility::Hidden,
            capabilities: CapabilitySet::empty(),
        }
    }

    /// Returns entry visibility independently from operation permissions.
    #[must_use]
    pub const fn visibility(self) -> EntryVisibility {
        self.visibility
    }

    /// Returns the additive effective capabilities.
    #[must_use]
    pub const fn capabilities(self) -> CapabilitySet {
        self.capabilities
    }

    /// Returns whether the operation is authorized.
    #[must_use]
    pub const fn allows(self, capability: Capability) -> bool {
        self.capabilities.contains(capability)
    }
}

/// Evaluates additive Briefcase authorization for one entry.
///
/// Rights come from exactly four places:
///
/// - **the drive**: its owner, and the custodian of a Silicon that owns it,
///   hold every operation on every file and folder in it, including what
///   others created there (it is their space and their quota);
/// - **creating the entry**: the creator keeps every operation on it, and so
///   does the custodian of a Silicon that created it (the custodian rule);
/// - **an explicit share** on the entry or an inheriting ancestor;
/// - **creating a folder above it**, which conveys reading what others put
///   inside.
///
/// Nothing is open to "everyone": the Public and Tag boundaries of rows from
/// before personal drives convey nothing. An entry in another drive than the
/// one the request acts in is always hidden; callers must still enforce
/// tree-wide checks for recursive operations.
#[must_use]
pub fn evaluate_authorization(input: &EffectiveAuthorizationInput<'_>) -> EffectiveAuthorization {
    if input.context.organization_id() != input.entry_organization_id {
        return EffectiveAuthorization::hidden();
    }

    let mut capabilities = CapabilitySet::empty();
    let has_owner_rights = input.context.has_owner_rights();
    // The owner column of a pre-migration organization container names
    // whoever first materialized it, not a proprietor; the apps container
    // belongs to the drive and is reached through the drive's rights.
    let ownership_is_authoritative = !matches!(
        input.system_kind,
        Some(
            SystemEntryKind::PublicContainer
                | SystemEntryKind::PrivateContainer
                | SystemEntryKind::TagRoot
                | SystemEntryKind::ApplicationContainer
        )
    );
    if has_owner_rights || (input.context.is_creator(input.owner) && ownership_is_authoritative) {
        capabilities = CapabilitySet::all();
    }

    // Creating a folder means seeing what is inside it. Without this, a
    // Carbon who invites someone into their folder loses sight of whatever
    // that person puts there. It conveys reading only: renaming, replacing
    // and deleting stay with whoever created the entry.
    if input.owns_ancestor {
        capabilities.insert(Capability::Read);
    }

    // System entries cannot receive grants. Ignoring them here makes the
    // invariant defensive even if corrupt persistence data reaches policy.
    if input.system_kind.is_none() {
        for application in input.grants.iter().copied() {
            let grant = application.grant();
            if grant.organization_id() == input.entry_organization_id
                && grant.principal() == input.context.actor()
                && application.applies_to(input.entry_id)
            {
                capabilities.union_with(CapabilitySet::from_granted_access(grant.access()));
            }
        }
    }

    if !input.entry_kind.can_contain_children() {
        capabilities.remove(Capability::CreateChild);
    }
    if input.entry_kind == EntryKind::Folder {
        capabilities.remove(Capability::WriteContent);
    }

    if matches!(
        input.system_kind,
        Some(SystemEntryKind::ApplicationContainer | SystemEntryKind::PrivateContainer)
    ) {
        // Only Briefcase creates the folders directly inside these.
        capabilities.remove(Capability::CreateChild);
    }

    if input.system_kind.is_some() {
        // System folders are structure, not content: nobody renames, moves,
        // deletes, or shares them, the drive's owner included. Everything
        // inside them is ordinary content.
        capabilities.remove(Capability::UpdateMetadata);
        capabilities.remove(Capability::WriteContent);
        capabilities.remove(Capability::Delete);
        capabilities.remove(Capability::ManagePermissions);
    } else if has_owner_rights {
        // Restated so that no rule above quietly narrows the drive's rights.
        capabilities = CapabilitySet::all();
        if !input.entry_kind.can_contain_children() {
            capabilities.remove(Capability::CreateChild);
        }
        if input.entry_kind == EntryKind::Folder {
            capabilities.remove(Capability::WriteContent);
        }
    }

    let visibility = if capabilities.contains(Capability::Read) {
        EntryVisibility::Full
    } else if input.required_for_traversal && input.entry_kind == EntryKind::Folder {
        EntryVisibility::Traversal
    } else {
        EntryVisibility::Hidden
    };

    EffectiveAuthorization {
        visibility,
        capabilities,
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::{
        AccessRight, Capability, EffectiveAccess, EffectiveAuthorization,
        EffectiveAuthorizationInput, EntryVisibility, GrantApplication, GrantedAccess,
        PermissionGrant, PermissionGrantParts, PermissionInheritance, evaluate_authorization,
    };
    use crate::domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, DriveAccess,
            OrganizationId, RequestAuthContext,
        },
        entry::{EntryKind, SystemEntryKind},
        ids::{EntryId, GrantId},
    };

    fn account(kind: ActorKind, value: &str) -> ActorRef {
        ActorRef::new(
            kind,
            ActorId::new(value).unwrap_or_else(|error| panic!("test id {value}: {error}")),
        )
    }

    fn carbon(value: &str) -> ActorRef {
        account(ActorKind::Carbon, value)
    }

    fn silicon(value: &str) -> ActorRef {
        account(ActorKind::Silicon, value)
    }

    /// Ada's drive, the drive every test entry below lives in.
    fn drive() -> OrganizationId {
        OrganizationId::new("Ada1").unwrap_or_else(|error| panic!("test drive: {error}"))
    }

    fn in_adas_drive(caller: &ActorRef, access: DriveAccess) -> RequestAuthContext {
        RequestAuthContext::for_drive(
            carbon("Ada1"),
            caller.clone(),
            access,
            AuthenticationMode::Bearer,
        )
        .unwrap_or_else(|error| panic!("test context: {error}"))
    }

    struct Target<'a> {
        kind: EntryKind,
        system_kind: Option<SystemEntryKind>,
        owner: ActorRef,
        grants: &'a [GrantApplication<'a>],
        owns_ancestor: bool,
        required_for_traversal: bool,
        id: EntryId,
    }

    impl Target<'_> {
        fn file(owner: ActorRef) -> Self {
            Target {
                kind: EntryKind::File,
                system_kind: None,
                owner,
                grants: &[],
                owns_ancestor: false,
                required_for_traversal: false,
                id: EntryId::new(),
            }
        }

        fn folder(owner: ActorRef) -> Self {
            Target {
                kind: EntryKind::Folder,
                ..Self::file(owner)
            }
        }
    }

    fn evaluate(context: &RequestAuthContext, target: &Target<'_>) -> EffectiveAuthorization {
        evaluate_authorization(&EffectiveAuthorizationInput {
            context,
            entry_organization_id: &drive(),
            entry_id: target.id,
            entry_kind: target.kind,
            system_kind: target.system_kind,
            owner: &target.owner,
            origin_application_id: None,
            grants: target.grants,
            owns_ancestor: target.owns_ancestor,
            required_for_traversal: target.required_for_traversal,
        })
    }

    fn grant(
        principal: &ActorRef,
        entry_id: EntryId,
        access: GrantedAccess,
        inheritance: PermissionInheritance,
    ) -> PermissionGrant {
        PermissionGrant::from_parts(PermissionGrantParts {
            id: GrantId::new(),
            organization_id: drive(),
            entry_id,
            principal: principal.clone(),
            access,
            inheritance,
            granted_by: carbon("Ada1"),
            created_at: OffsetDateTime::UNIX_EPOCH,
        })
    }

    const EVERYTHING_ON_A_FOLDER: [EffectiveAccess; 5] = [
        EffectiveAccess::Read,
        EffectiveAccess::Write,
        EffectiveAccess::Update,
        EffectiveAccess::Delete,
        EffectiveAccess::ManagePermissions,
    ];

    #[test]
    fn the_drive_owner_holds_every_operation_on_everything_in_the_drive() {
        let ada = carbon("Ada1");
        let owner = in_adas_drive(&ada, DriveAccess::Owner);
        // Bob created this folder inside Ada's drive through a share.
        let authorization = evaluate(&owner, &Target::folder(carbon("Bob1")));
        assert_eq!(authorization.visibility(), EntryVisibility::Full);
        assert_eq!(
            authorization.capabilities().effective_access(),
            EVERYTHING_ON_A_FOLDER
        );
    }

    #[test]
    fn a_custodian_holds_the_owner_rights_in_its_silicons_drive() {
        let ada = carbon("Ada1");
        let silicon_drive = RequestAuthContext::for_drive(
            silicon("Ada1"),
            carbon("Cus1"),
            DriveAccess::Custodian,
            AuthenticationMode::Bearer,
        )
        .unwrap_or_else(|error| panic!("test context: {error}"));
        let authorization = evaluate(&silicon_drive, &Target::folder(ada));
        assert_eq!(
            authorization.capabilities().effective_access(),
            EVERYTHING_ON_A_FOLDER
        );
    }

    #[test]
    fn a_visitor_sees_nothing_that_was_not_shared() {
        let visitor = in_adas_drive(&carbon("Bob1"), DriveAccess::Visitor);
        let authorization = evaluate(&visitor, &Target::file(carbon("Ada1")));
        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }

    #[test]
    fn creators_keep_their_rights_in_another_accounts_drive() {
        let bob = carbon("Bob1");
        let visitor = in_adas_drive(&bob, DriveAccess::Visitor);
        let own = evaluate(&visitor, &Target::file(bob.clone()));
        assert!(own.allows(Capability::Delete));
        assert!(own.allows(Capability::ManagePermissions));
        let sibling = evaluate(&visitor, &Target::file(carbon("Ada1")));
        assert_eq!(sibling.visibility(), EntryVisibility::Hidden);
    }

    #[test]
    fn a_custodian_has_creator_rights_on_what_its_silicons_created_elsewhere() {
        let scout = silicon("Sc1");
        let custodian = in_adas_drive(&carbon("Cus1"), DriveAccess::Visitor)
            .with_custodial_silicons([scout.id().clone()]);
        let created_by_scout = evaluate(&custodian, &Target::file(scout));
        assert!(created_by_scout.allows(Capability::Read));
        assert!(created_by_scout.allows(Capability::Delete));
        let created_by_ada = evaluate(&custodian, &Target::file(carbon("Ada1")));
        assert_eq!(created_by_ada.visibility(), EntryVisibility::Hidden);
    }

    #[test]
    fn the_drive_root_admits_new_entries_from_the_owner_rights_only() {
        let ada = carbon("Ada1");
        let root = Target {
            system_kind: Some(SystemEntryKind::DriveRoot),
            ..Target::folder(ada.clone())
        };
        let owner = evaluate(&in_adas_drive(&ada, DriveAccess::Owner), &root);
        assert!(owner.allows(Capability::Read) && owner.allows(Capability::CreateChild));
        assert!(!owner.allows(Capability::UpdateMetadata));
        assert!(!owner.allows(Capability::Delete));
        assert!(!owner.allows(Capability::ManagePermissions));

        let visitor = in_adas_drive(&carbon("Bob1"), DriveAccess::Visitor);
        assert_eq!(
            evaluate(&visitor, &root).visibility(),
            EntryVisibility::Hidden
        );
        let traversed = Target {
            required_for_traversal: true,
            ..root
        };
        let traversal = evaluate(&visitor, &traversed);
        assert_eq!(traversal.visibility(), EntryVisibility::Traversal);
        assert!(!traversal.allows(Capability::CreateChild));
    }

    #[test]
    fn apps_folders_are_structure_the_owner_reads_and_apps_write_into() {
        let ada = carbon("Ada1");
        let owner = in_adas_drive(&ada, DriveAccess::Owner);
        let container = evaluate(
            &owner,
            &Target {
                system_kind: Some(SystemEntryKind::ApplicationContainer),
                ..Target::folder(ada.clone())
            },
        );
        assert!(container.allows(Capability::Read));
        assert!(!container.allows(Capability::CreateChild));
        let app_folder = evaluate(
            &owner,
            &Target {
                system_kind: Some(SystemEntryKind::ApplicationFolder),
                ..Target::folder(ada)
            },
        );
        assert!(app_folder.allows(Capability::CreateChild));
        assert!(!app_folder.allows(Capability::Delete));
        assert!(!app_folder.allows(Capability::ManagePermissions));
    }

    #[test]
    fn legacy_public_and_tag_containers_convey_nothing() {
        let visitor = in_adas_drive(&carbon("Bob1"), DriveAccess::Visitor);
        for system_kind in [
            SystemEntryKind::PublicContainer,
            SystemEntryKind::TagRoot,
            SystemEntryKind::PrivateContainer,
        ] {
            let authorization = evaluate(
                &visitor,
                &Target {
                    system_kind: Some(system_kind),
                    ..Target::folder(carbon("Bob1"))
                },
            );
            assert_eq!(
                authorization.visibility(),
                EntryVisibility::Hidden,
                "{system_kind:?}"
            );
        }
    }

    #[test]
    fn an_inherited_update_grant_conveys_neither_deletion_nor_management() {
        let bob = carbon("Bob1");
        let source = EntryId::new();
        let shared = grant(
            &bob,
            source,
            GrantedAccess::new([AccessRight::Update]),
            PermissionInheritance::Descendants,
        );
        let grants = [GrantApplication::Inherited(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..Target::file(carbon("Ada1"))
            },
        );
        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::WriteContent));
        assert!(authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
        assert!(!authorization.allows(Capability::ManagePermissions));
        assert_eq!(
            authorization.capabilities().effective_access(),
            vec![EffectiveAccess::Read, EffectiveAccess::Update]
        );
    }

    #[test]
    fn deletion_requires_its_own_right_even_alongside_update() {
        let bob = carbon("Bob1");
        let target = Target::file(carbon("Ada1"));
        let shared = grant(
            &bob,
            target.id,
            GrantedAccess::new([AccessRight::Update, AccessRight::Delete]),
            PermissionInheritance::EntryOnly,
        );
        let grants = [GrantApplication::Direct(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..target
            },
        );
        assert!(!authorization.allows(Capability::Delete));
        assert!(authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::ManagePermissions));
    }

    #[test]
    fn a_write_grant_adds_children_without_renaming_or_deleting() {
        let scout = silicon("Sc1");
        let target = Target::folder(carbon("Ada1"));
        let shared = grant(
            &scout,
            target.id,
            GrantedAccess::new([AccessRight::Write]),
            PermissionInheritance::EntryOnly,
        );
        let grants = [GrantApplication::Direct(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&scout, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..target
            },
        );
        assert!(authorization.allows(Capability::CreateChild));
        assert!(!authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
    }

    #[test]
    fn non_inheritable_grants_do_not_flow_to_descendants() {
        let bob = carbon("Bob1");
        let shared = grant(
            &bob,
            EntryId::new(),
            GrantedAccess::READ_ONLY,
            PermissionInheritance::EntryOnly,
        );
        let grants = [GrantApplication::Inherited(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..Target::file(carbon("Ada1"))
            },
        );
        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }

    #[test]
    fn a_share_to_someone_else_conveys_nothing_to_the_caller() {
        let bob = carbon("Bob1");
        let target = Target::file(carbon("Ada1"));
        let shared = grant(
            &carbon("Cat1"),
            target.id,
            GrantedAccess::READ_ONLY,
            PermissionInheritance::EntryOnly,
        );
        let grants = [GrantApplication::Direct(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..target
            },
        );
        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
    }

    #[test]
    fn delegated_authority_is_independent_of_the_entries_origin() {
        let ada = carbon("Ada1");
        let delegated = RequestAuthContext::for_drive(
            ada.clone(),
            ada.clone(),
            DriveAccess::Owner,
            AuthenticationMode::OnBehalfOf {
                application_id: ApplicationId::new("waveform")
                    .unwrap_or_else(|error| panic!("test app: {error}")),
            },
        )
        .unwrap_or_else(|error| panic!("test context: {error}"));
        let other_application =
            ApplicationId::new("extend").unwrap_or_else(|error| panic!("test app: {error}"));
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &delegated,
            entry_organization_id: &drive(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::File,
            system_kind: None,
            owner: &ada,
            origin_application_id: Some(&other_application),
            grants: &[],
            owns_ancestor: false,
            required_for_traversal: false,
        });
        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::WriteContent));
        assert!(authorization.allows(Capability::Delete));
    }

    #[test]
    fn an_entry_in_another_drive_is_hidden_even_from_its_owner() {
        let ada = carbon("Ada1");
        let in_bobs_drive = RequestAuthContext::for_drive(
            carbon("Bob1"),
            ada.clone(),
            DriveAccess::Visitor,
            AuthenticationMode::Bearer,
        )
        .unwrap_or_else(|error| panic!("test context: {error}"));
        let authorization = evaluate(&in_bobs_drive, &Target::file(ada));
        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }

    #[test]
    fn folder_write_never_replaces_a_file_that_already_exists() {
        let bob = carbon("Bob1");
        let shared = grant(
            &bob,
            EntryId::new(),
            GrantedAccess::new([AccessRight::Write]),
            PermissionInheritance::Descendants,
        );
        let grants = [GrantApplication::Inherited(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..Target::file(carbon("Ada1"))
            },
        );
        // Write adds what is not there yet. Replacing the bytes of someone
        // else's existing file is an update, and this grant never said so.
        assert!(authorization.allows(Capability::Read));
        assert!(!authorization.allows(Capability::WriteContent));
        assert!(!authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
    }

    #[test]
    fn update_conveys_replacing_the_content_of_an_existing_file() {
        let bob = carbon("Bob1");
        let target = Target::file(carbon("Ada1"));
        let shared = grant(
            &bob,
            target.id,
            GrantedAccess::new([AccessRight::Update]),
            PermissionInheritance::EntryOnly,
        );
        let grants = [GrantApplication::Direct(&shared)];
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                grants: &grants,
                ..target
            },
        );
        assert!(authorization.allows(Capability::WriteContent));
        assert!(authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
    }

    #[test]
    fn creating_a_folder_shows_what_others_put_inside_it_and_nothing_more() {
        let bob = carbon("Bob1");
        let authorization = evaluate(
            &in_adas_drive(&bob, DriveAccess::Visitor),
            &Target {
                owns_ancestor: true,
                ..Target::file(carbon("Cat1"))
            },
        );
        assert_eq!(authorization.visibility(), EntryVisibility::Full);
        assert!(authorization.allows(Capability::Read));
        assert!(!authorization.allows(Capability::WriteContent));
        assert!(!authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
        assert!(!authorization.allows(Capability::ManagePermissions));
    }
}
