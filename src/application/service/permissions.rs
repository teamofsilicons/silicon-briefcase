//! Explicit permission grant use cases.

use crate::{
    application::context::ExecutionContext,
    domain::{
        ids::EntryId,
        permission::{Capability, EntryVisibility, PermissionGrant},
    },
};

use super::{
    AuthorizedEntryView, GrantPermissionCommand, InspectPermissionsQuery, ListPermissionsQuery,
    MetadataService, MetadataServiceError, MutationMetadata, Page, RevokePermissionCommand,
    ValidationError, require_capability, validate_context,
};

impl MetadataService {
    /// Lists explicit grants for an entry the caller may administer.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context or target is
    /// invalid, permission management is unauthorized, or repository access
    /// fails.
    pub async fn list_permissions(
        &self,
        context: &ExecutionContext,
        query: &ListPermissionsQuery,
    ) -> Result<Page<PermissionGrant>, MetadataServiceError> {
        validate_context(context)?;
        let entry = self.load_permission_target(context, query.entry_id).await?;
        require_capability(&entry, context, Capability::ManagePermissions)?;
        let page = self
            .repository
            .list_permission_grants(context, query)
            .await?;
        self.repository
            .record_metadata_access(context, &[query.entry_id])
            .await?;
        Ok(page)
    }

    /// Reports what the caller may do on each requested file or folder.
    ///
    /// Targets the caller cannot read are simply absent from the result, so
    /// the answer never distinguishes "hidden" from "does not exist".
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// repository access or auditing fails.
    pub async fn inspect_permissions(
        &self,
        context: &ExecutionContext,
        query: &InspectPermissionsQuery,
    ) -> Result<Vec<AuthorizedEntryView>, MetadataServiceError> {
        validate_context(context)?;
        let candidates = self
            .repository
            .find_active_entries(context, &query.entry_ids, &query.paths)
            .await?;
        let mut visible = Vec::with_capacity(candidates.len());
        let mut accessed = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let authorization = candidate.authorization(context.authorization());
            if authorization.visibility() != EntryVisibility::Full {
                continue;
            }
            accessed.push(candidate.entry.id);
            visible.push(AuthorizedEntryView {
                entry: candidate.entry,
                effective_access: authorization.capabilities().effective_access(),
            });
        }
        if !accessed.is_empty() {
            self.repository
                .record_metadata_access(context, &accessed)
                .await?;
        }
        Ok(visible)
    }

    /// Grants a current organization member read or write access.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when validation fails, the target is
    /// unavailable, permission management is unauthorized, the principal is
    /// not a current member, or persistence fails.
    pub async fn grant_permission(
        &self,
        context: &ExecutionContext,
        command: &GrantPermissionCommand,
        metadata: &MutationMetadata,
    ) -> Result<PermissionGrant, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .load_permission_target(context, command.entry_id)
            .await?;
        require_capability(&entry, context, Capability::ManagePermissions)?;
        if !self
            .repository
            .is_current_member(context, &command.principal)
            .await?
        {
            return Err(ValidationError {
                field: "principal",
                message: "must be a current organization member",
            }
            .into());
        }
        self.repository
            .grant_permission(context, command, metadata, Capability::ManagePermissions)
            .await
            .map_err(Into::into)
    }

    /// Revokes one direct grant without affecting independent access sources.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context, target, or
    /// grant is invalid, permission management is unauthorized, or persistence
    /// fails.
    pub async fn revoke_permission(
        &self,
        context: &ExecutionContext,
        command: RevokePermissionCommand,
        metadata: &MutationMetadata,
    ) -> Result<(), MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .load_permission_target(context, command.entry_id)
            .await?;
        require_capability(&entry, context, Capability::ManagePermissions)?;
        if !self
            .repository
            .grant_exists(context, command.entry_id, command.grant_id)
            .await?
        {
            return Err(MetadataServiceError::NotFound);
        }
        self.repository
            .revoke_permission(context, command, metadata, Capability::ManagePermissions)
            .await?;
        Ok(())
    }

    async fn load_permission_target(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<super::AuthorizableEntry, MetadataServiceError> {
        self.repository
            .find_active_entry(context, entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)
    }
}
