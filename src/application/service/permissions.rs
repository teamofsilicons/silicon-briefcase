//! Explicit permission grant use cases.

use crate::{
    application::context::ExecutionContext,
    domain::{
        ids::EntryId,
        permission::{Capability, PermissionGrant},
    },
};

use super::{
    GrantPermissionCommand, ListPermissionsQuery, MetadataService, MetadataServiceError,
    MutationMetadata, Page, RevokePermissionCommand, ValidationError, require_capability,
    validate_context,
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
