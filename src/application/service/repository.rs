//! Repository and version-restoration ports consumed by metadata services.

use async_trait::async_trait;
use thiserror::Error;

use crate::{
    application::context::ExecutionContext,
    domain::{
        actor::{ActorRef, OrganizationId},
        entry::{EntryBoundary, EntryPath},
        ids::{AccessRequestId, EntryId, GrantId},
        notification::NotificationInbox,
        permission::{Capability, PermissionGrant},
        quota::OrganizationUsage,
    },
};

use super::model::{
    AccessRequestView, ActivityEvent, AuthorizableAccessRequest, AuthorizableEntry,
    CreateFolderMutation, DecideAccessRequestCommand, FileVersionView, GrantPermissionCommand,
    ListBinQuery, ListEntriesQuery, ListPermissionsQuery, ListVersionsQuery, MutationMetadata,
    Page, ProjectedMembership, RequestAccessCommand, RevokePermissionCommand, SearchCandidate,
    SearchQuery, UpdateEntryCommand,
};

/// Persistence failures classified without exposing SQL or tenant details.
#[derive(Debug, Error)]
pub enum MetadataRepositoryError {
    /// Target is absent in the current organization.
    #[error("metadata resource not found")]
    NotFound,
    /// Uniqueness, state, or optimistic authorization changed concurrently.
    #[error("metadata state conflict")]
    Conflict,
    /// Supplied pagination cursor is not one this repository issued.
    #[error("pagination cursor is invalid")]
    InvalidCursor,
    /// Repository cannot serve the operation before its deadline.
    #[error("metadata repository unavailable")]
    Unavailable,
    /// Unexpected adapter failure retained as a source for internal telemetry.
    #[error("internal metadata repository failure")]
    Internal(#[source] anyhow::Error),
}

/// Metadata persistence required by contracted application use cases.
///
/// Mutation implementations must execute the state change, audit record,
/// idempotency result, and outbox events atomically. They must lock and
/// re-evaluate `required_capability` with the supplied current IAM context so a
/// permission change between the service read and write cannot authorize a
/// stale mutation.
#[async_trait]
pub trait MetadataRepository: Send + Sync {
    /// Loads an active entry and all domain authorization facts.
    async fn find_active_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Option<AuthorizableEntry>, MetadataRepositoryError>;

    /// Loads the reserved container a boundary's content belongs in.
    ///
    /// Public and Tag resolve to their own container; Private resolves to the
    /// caller's own folder inside it, never to the shared Private container.
    /// Resolution is a lookup: the caller still evaluates domain policy, which
    /// is what keeps a tag the caller does not carry out of reach.
    async fn find_boundary_container(
        &self,
        context: &ExecutionContext,
        boundary: &EntryBoundary,
    ) -> Result<Option<AuthorizableEntry>, MetadataRepositoryError>;

    /// Loads an active entry addressed by its organization-relative path.
    ///
    /// Path resolution is a lookup, not an authorization decision: the caller
    /// still evaluates domain policy and answers not-found for a hidden entry.
    async fn find_active_entry_by_path(
        &self,
        context: &ExecutionContext,
        path: &EntryPath,
    ) -> Result<Option<AuthorizableEntry>, MetadataRepositoryError>;

    /// Loads every active entry addressed by an identifier or a path.
    ///
    /// Resolving a batch in one transaction keeps a permission inspection of
    /// many targets to a single consistent snapshot.
    async fn find_active_entries(
        &self,
        context: &ExecutionContext,
        entry_ids: &[EntryId],
        paths: &[EntryPath],
    ) -> Result<Vec<AuthorizableEntry>, MetadataRepositoryError>;

    /// Lists tenant-local active child candidates in stable cursor order.
    async fn list_active_children(
        &self,
        context: &ExecutionContext,
        query: &ListEntriesQuery,
    ) -> Result<Page<AuthorizableEntry>, MetadataRepositoryError>;

    /// Atomically creates a folder after rechecking parent/root authority.
    async fn create_folder(
        &self,
        context: &ExecutionContext,
        mutation: &CreateFolderMutation,
        metadata: &MutationMetadata,
        required_parent_capability: Option<Capability>,
    ) -> Result<AuthorizableEntry, MetadataRepositoryError>;

    /// Atomically renames or moves an entry and rejects tree cycles.
    async fn update_entry(
        &self,
        context: &ExecutionContext,
        command: &UpdateEntryCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AuthorizableEntry, MetadataRepositoryError>;

    /// Atomically marks a complete subtree recoverable for 45 days.
    async fn soft_delete_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<(), MetadataRepositoryError>;

    /// Lists explicit, non-revoked grants for an entry.
    async fn list_permission_grants(
        &self,
        context: &ExecutionContext,
        query: &ListPermissionsQuery,
    ) -> Result<Page<PermissionGrant>, MetadataRepositoryError>;

    /// Atomically creates an explicit grant after membership revalidation.
    async fn grant_permission(
        &self,
        context: &ExecutionContext,
        command: &GrantPermissionCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<PermissionGrant, MetadataRepositoryError>;

    /// Atomically revokes a direct grant while preserving independent access.
    async fn revoke_permission(
        &self,
        context: &ExecutionContext,
        command: RevokePermissionCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<(), MetadataRepositoryError>;

    /// Creates a pending access request without returning target metadata.
    async fn create_access_request(
        &self,
        context: &ExecutionContext,
        command: &RequestAccessCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView, MetadataRepositoryError>;

    /// Loads a pending or decided request with its target authorization facts.
    async fn find_access_request(
        &self,
        context: &ExecutionContext,
        request_id: AccessRequestId,
    ) -> Result<Option<AuthorizableAccessRequest>, MetadataRepositoryError>;

    /// Atomically records a decision and creates an approval grant.
    async fn decide_access_request(
        &self,
        context: &ExecutionContext,
        command: DecideAccessRequestCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AccessRequestView, MetadataRepositoryError>;

    /// Finds already permission-filtered candidates; service policy rechecks them.
    async fn search(
        &self,
        context: &ExecutionContext,
        query: &SearchQuery,
    ) -> Result<Vec<SearchCandidate>, MetadataRepositoryError>;

    /// Lists retained versions of a current file.
    async fn list_file_versions(
        &self,
        context: &ExecutionContext,
        query: &ListVersionsQuery,
    ) -> Result<Page<FileVersionView>, MetadataRepositoryError>;

    /// Lists recoverable subtree roots eligible for the actor's bin.
    async fn list_bin_entries(
        &self,
        context: &ExecutionContext,
        query: &ListBinQuery,
    ) -> Result<Page<AuthorizableEntry>, MetadataRepositoryError>;

    /// Loads one recoverable subtree root with current authorization facts.
    async fn find_bin_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Option<AuthorizableEntry>, MetadataRepositoryError>;

    /// Atomically restores a retained subtree, applying deterministic fallback naming.
    async fn restore_bin_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AuthorizableEntry, MetadataRepositoryError>;

    /// Reads the projected organization role and tags of one member.
    ///
    /// IAM's OBO result names the represented actor but carries no role or
    /// tags, so an application request derives them from Briefcase's own IAM
    /// projection instead of assuming them.
    async fn project_member_authorization(
        &self,
        organization_id: &OrganizationId,
        actor: &ActorRef,
        request_id: &str,
    ) -> Result<Option<ProjectedMembership>, MetadataRepositoryError>;

    /// Materializes the calling application's folder for the represented actor.
    async fn ensure_application_folder(
        &self,
        context: &ExecutionContext,
    ) -> Result<AuthorizableEntry, MetadataRepositoryError>;

    /// Lists the retained action history of one entry, newest first.
    async fn list_entry_activity(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Vec<ActivityEvent>, MetadataRepositoryError>;

    /// Loads the caller's newest notifications and unread badge count.
    async fn load_notification_inbox(
        &self,
        context: &ExecutionContext,
    ) -> Result<NotificationInbox, MetadataRepositoryError>;

    /// Reads what the organization consumes and the limits it consumes against.
    async fn load_organization_usage(
        &self,
        context: &ExecutionContext,
    ) -> Result<OrganizationUsage, MetadataRepositoryError>;

    /// Marks the caller's complete inbox read and returns it afterwards.
    async fn mark_notifications_read(
        &self,
        context: &ExecutionContext,
        metadata: &MutationMetadata,
    ) -> Result<NotificationInbox, MetadataRepositoryError>;

    /// Records successful metadata reads for the required audit history.
    async fn record_metadata_access(
        &self,
        context: &ExecutionContext,
        entry_ids: &[EntryId],
    ) -> Result<(), MetadataRepositoryError>;

    /// Confirms that a target principal is a current member of the organization.
    async fn is_current_member(
        &self,
        context: &ExecutionContext,
        principal: &crate::domain::actor::ActorRef,
    ) -> Result<bool, MetadataRepositoryError>;

    /// Confirms a grant is direct, current, and belongs to the target entry.
    async fn grant_exists(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        grant_id: GrantId,
    ) -> Result<bool, MetadataRepositoryError>;
}
