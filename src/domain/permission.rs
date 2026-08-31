//! Additive entry authorization and explicit permission grants.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    actor::{ActorRef, ApplicationId, OrganizationId, RequestAuthContext},
    entry::{EntryBoundary, EntryKind, SystemEntryKind},
    ids::{EntryId, GrantId},
};

/// The v1 access level accepted when granting or requesting access.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    /// Read access only.
    Read,
    /// Read and mutation access, excluding permission management.
    Write,
}

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
    /// Organization security boundary.
    pub organization_id: OrganizationId,
    /// Entry on which the grant was created.
    pub entry_id: EntryId,
    /// Current organization member receiving the grant.
    pub principal: ActorRef,
    /// Granted v1 access level.
    pub access: AccessLevel,
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
}

impl PermissionGrant {
    /// Rehydrates or creates a grant from already validated facts.
    #[must_use]
    pub const fn from_parts(parts: PermissionGrantParts) -> Self {
        Self { parts }
    }

    /// Returns the grant identifier.
    #[must_use]
    pub const fn id(&self) -> GrantId {
        self.parts.id
    }

    /// Returns the organization security boundary.
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

    /// Returns the granted access level.
    #[must_use]
    pub const fn access(&self) -> AccessLevel {
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
/// The public API still exposes only `read`, `write`, `delete`, and
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
    /// Create or restore file content.
    WriteContent,
    /// Move an entry to the recoverable bin.
    Delete,
    /// Create and revoke explicit grants.
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

    /// Returns the capabilities derived by a v1 access level.
    #[must_use]
    pub const fn from_access_level(access: AccessLevel) -> Self {
        match access {
            AccessLevel::Read => Self(Capability::Read.mask()),
            AccessLevel::Write => Self(
                Capability::Read.mask()
                    | Capability::CreateChild.mask()
                    | Capability::UpdateMetadata.mask()
                    | Capability::WriteContent.mask()
                    | Capability::Delete.mask(),
            ),
        }
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

    /// Maps fine-grained policy to the stable v1 effective-access vocabulary.
    #[must_use]
    pub fn effective_access(self) -> Vec<EffectiveAccess> {
        let mut access = Vec::with_capacity(4);
        if self.contains(Capability::Read) {
            access.push(EffectiveAccess::Read);
        }
        if self.contains(Capability::CreateChild)
            || self.contains(Capability::UpdateMetadata)
            || self.contains(Capability::WriteContent)
        {
            access.push(EffectiveAccess::Write);
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
    /// Read access.
    Read,
    /// At least one context-appropriate write operation.
    Write,
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
    /// Online-IAM-verified request identity and organization membership.
    pub context: &'a RequestAuthContext,
    /// Organization that owns the entry.
    pub entry_organization_id: &'a OrganizationId,
    /// Entry being authorized.
    pub entry_id: EntryId,
    /// Whether the target is a file or folder.
    pub entry_kind: EntryKind,
    /// Internal system classification, when reconciled by Briefcase.
    pub system_kind: Option<SystemEntryKind>,
    /// Inherited Public, Private, or Tag boundary.
    pub boundary: &'a EntryBoundary,
    /// Represented actor that owns the entry.
    pub owner: &'a ActorRef,
    /// Server-derived application that originally created the entry.
    pub origin_application_id: Option<&'a ApplicationId>,
    /// Direct and repository-proven ancestor grants relevant to the entry.
    pub grants: &'a [GrantApplication<'a>],
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
/// A cross-organization entry always evaluates to hidden with no capabilities.
/// The caller must still enforce tree-wide checks for recursive operations.
#[must_use]
pub fn evaluate_authorization(input: &EffectiveAuthorizationInput<'_>) -> EffectiveAuthorization {
    if input.context.organization_id() != input.entry_organization_id {
        return EffectiveAuthorization {
            visibility: EntryVisibility::Hidden,
            capabilities: CapabilitySet::empty(),
        };
    }

    let mut capabilities = CapabilitySet::empty();
    let is_owner = input.context.actor() == input.owner;
    let is_administrator = input.context.role().has_administrative_access();
    let ownership_is_authoritative = !matches!(
        input.system_kind,
        Some(
            SystemEntryKind::PublicContainer
                | SystemEntryKind::PrivateContainer
                | SystemEntryKind::TagRoot
        )
    );

    if (is_owner && ownership_is_authoritative) || is_administrator {
        capabilities = CapabilitySet::all();
    }

    match input.boundary {
        EntryBoundary::Public => {
            capabilities.insert(Capability::Read);
            if input.entry_kind.can_contain_children() {
                capabilities.insert(Capability::CreateChild);
            }
        }
        EntryBoundary::Tag { tag } if input.context.has_tag(tag) => {
            capabilities.insert(Capability::Read);
        }
        EntryBoundary::Private | EntryBoundary::Tag { .. } => {}
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
                capabilities.union_with(CapabilitySet::from_access_level(grant.access()));
            }
        }
    }

    if !input.entry_kind.can_contain_children() {
        capabilities.remove(Capability::CreateChild);
    }
    if input.entry_kind == EntryKind::Folder {
        capabilities.remove(Capability::WriteContent);
    }

    if input.system_kind == Some(SystemEntryKind::PrivateContainer) {
        // Every current organization member must be able to enter the canonical
        // Private container. Only the reconciler creates actor folders directly
        // below it, so its persistence custodian is not allowed to create
        // arbitrary children through the public API.
        capabilities.insert(Capability::Read);
        capabilities.remove(Capability::CreateChild);
    }

    if input.system_kind.is_some() {
        capabilities.remove(Capability::UpdateMetadata);
        capabilities.remove(Capability::WriteContent);
        capabilities.remove(Capability::Delete);
        capabilities.remove(Capability::ManagePermissions);
    }

    if let Some(application_id) = input.context.originating_application()
        && input.origin_application_id != Some(application_id)
    {
        capabilities.remove(Capability::Delete);
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
        AccessLevel, Capability, EffectiveAuthorizationInput, EntryVisibility, GrantApplication,
        PermissionGrant, PermissionGrantParts, PermissionInheritance, evaluate_authorization,
    };
    use crate::domain::{
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, OrganizationId,
            OrganizationRole, RequestAuthContext, TagName,
        },
        entry::{EntryBoundary, EntryKind, SystemEntryKind},
        ids::{EntryId, GrantId},
    };

    fn external_id<T, E>(result: Result<T, E>) -> T
    where
        E: std::fmt::Display,
    {
        match result {
            Ok(value) => value,
            Err(error) => panic!("test identifier must be valid: {error}"),
        }
    }

    fn actor(value: &str) -> ActorRef {
        ActorRef::new(ActorKind::Carbon, external_id(ActorId::new(value)))
    }

    fn organization() -> OrganizationId {
        external_id(OrganizationId::new("tos"))
    }

    fn context(
        represented_actor: ActorRef,
        role: OrganizationRole,
        tags: Vec<TagName>,
        authentication: AuthenticationMode,
    ) -> RequestAuthContext {
        RequestAuthContext::new(
            organization(),
            represented_actor,
            role,
            tags,
            authentication,
        )
    }

    #[test]
    fn public_members_can_create_children_without_mutating_the_folder() {
        let caller = actor("carbon-a");
        let owner = actor("carbon-b");
        let context = context(
            caller,
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::Folder,
            system_kind: None,
            boundary: &EntryBoundary::Public,
            owner: &owner,
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::CreateChild));
        assert!(!authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
    }

    #[test]
    fn matching_tag_derives_read_only() {
        let finance = external_id(TagName::new("finance"));
        let context = context(
            actor("carbon-a"),
            OrganizationRole::Member,
            vec![finance.clone()],
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::File,
            system_kind: None,
            boundary: &EntryBoundary::Tag { tag: finance },
            owner: &actor("carbon-b"),
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert_eq!(authorization.visibility(), EntryVisibility::Full);
        assert!(authorization.allows(Capability::Read));
        assert!(!authorization.allows(Capability::WriteContent));
    }

    #[test]
    fn inheritable_write_grants_add_write_and_delete_but_not_management() {
        let caller = actor("carbon-a");
        let context = context(
            caller.clone(),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let source_entry_id = EntryId::new();
        let target_entry_id = EntryId::new();
        let grant = PermissionGrant::from_parts(PermissionGrantParts {
            id: GrantId::new(),
            organization_id: organization(),
            entry_id: source_entry_id,
            principal: caller,
            access: AccessLevel::Write,
            inheritance: PermissionInheritance::Descendants,
            granted_by: actor("carbon-owner"),
            created_at: OffsetDateTime::UNIX_EPOCH,
        });
        let grants = [GrantApplication::Inherited(&grant)];
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: target_entry_id,
            entry_kind: EntryKind::File,
            system_kind: None,
            boundary: &EntryBoundary::Private,
            owner: &actor("carbon-b"),
            origin_application_id: None,
            grants: &grants,
            required_for_traversal: false,
        });

        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::WriteContent));
        assert!(authorization.allows(Capability::UpdateMetadata));
        assert!(authorization.allows(Capability::Delete));
        assert!(!authorization.allows(Capability::ManagePermissions));
    }

    #[test]
    fn non_inheritable_grants_do_not_flow_to_descendants() {
        let caller = actor("carbon-a");
        let context = context(
            caller.clone(),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let grant = PermissionGrant::from_parts(PermissionGrantParts {
            id: GrantId::new(),
            organization_id: organization(),
            entry_id: EntryId::new(),
            principal: caller,
            access: AccessLevel::Read,
            inheritance: PermissionInheritance::EntryOnly,
            granted_by: actor("carbon-owner"),
            created_at: OffsetDateTime::UNIX_EPOCH,
        });
        let grants = [GrantApplication::Inherited(&grant)];
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::File,
            system_kind: None,
            boundary: &EntryBoundary::Private,
            owner: &actor("carbon-b"),
            origin_application_id: None,
            grants: &grants,
            required_for_traversal: false,
        });

        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }

    #[test]
    fn canonical_private_container_is_navigation_visible_to_every_member() {
        let context = context(
            actor("carbon-a"),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::Folder,
            system_kind: Some(SystemEntryKind::PrivateContainer),
            boundary: &EntryBoundary::Private,
            owner: &actor("carbon-b"),
            origin_application_id: None,
            grants: &[],
            required_for_traversal: true,
        });

        assert_eq!(authorization.visibility(), EntryVisibility::Full);
        assert!(authorization.allows(Capability::Read));
        assert!(!authorization.allows(Capability::CreateChild));
    }

    #[test]
    fn private_container_custodial_owner_cannot_create_arbitrary_children() {
        let custodian = actor("carbon-a");
        let context = context(
            custodian.clone(),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::Folder,
            system_kind: Some(SystemEntryKind::PrivateContainer),
            boundary: &EntryBoundary::Private,
            owner: &custodian,
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert!(authorization.allows(Capability::Read));
        assert!(!authorization.allows(Capability::CreateChild));
        assert!(!authorization.allows(Capability::UpdateMetadata));
    }

    #[test]
    fn tag_root_custodian_does_not_bypass_current_iam_tags() {
        let custodian = actor("carbon-a");
        let context = context(
            custodian.clone(),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::Folder,
            system_kind: Some(SystemEntryKind::TagRoot),
            boundary: &EntryBoundary::Tag {
                tag: external_id(TagName::new("finance")),
            },
            owner: &custodian,
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }

    #[test]
    fn system_entries_remain_immutable_even_for_administrators() {
        let administrator = actor("carbon-admin");
        let context = context(
            administrator.clone(),
            OrganizationRole::Admin,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::Folder,
            system_kind: Some(SystemEntryKind::PrivateActorFolder),
            boundary: &EntryBoundary::Private,
            owner: &administrator,
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::CreateChild));
        assert!(!authorization.allows(Capability::UpdateMetadata));
        assert!(!authorization.allows(Capability::Delete));
        assert!(!authorization.allows(Capability::ManagePermissions));
    }

    #[test]
    fn obo_application_cannot_delete_an_entry_created_by_another_origin() {
        let application_id = external_id(ApplicationId::new("silicon-dm"));
        let other_application_id = external_id(ApplicationId::new("silicon-remind"));
        let represented_actor = actor("carbon-a");
        let context = context(
            represented_actor.clone(),
            OrganizationRole::Member,
            Vec::new(),
            AuthenticationMode::OnBehalfOf {
                application_id: application_id.clone(),
            },
        );
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &organization(),
            entry_id: EntryId::new(),
            entry_kind: EntryKind::File,
            system_kind: None,
            boundary: &EntryBoundary::Private,
            owner: &represented_actor,
            origin_application_id: Some(&other_application_id),
            grants: &[],
            required_for_traversal: false,
        });

        assert!(authorization.allows(Capability::Read));
        assert!(authorization.allows(Capability::WriteContent));
        assert!(!authorization.allows(Capability::Delete));
    }

    #[test]
    fn tenant_mismatch_fails_closed() {
        let context = context(
            actor("carbon-a"),
            OrganizationRole::Owner,
            Vec::new(),
            AuthenticationMode::Bearer,
        );
        let other_organization = external_id(OrganizationId::new("other"));
        let authorization = evaluate_authorization(&EffectiveAuthorizationInput {
            context: &context,
            entry_organization_id: &other_organization,
            entry_id: EntryId::new(),
            entry_kind: EntryKind::File,
            system_kind: None,
            boundary: &EntryBoundary::Public,
            owner: context.actor(),
            origin_application_id: None,
            grants: &[],
            required_for_traversal: false,
        });

        assert_eq!(authorization.visibility(), EntryVisibility::Hidden);
        assert!(authorization.capabilities().is_empty());
    }
}
