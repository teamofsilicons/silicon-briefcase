//! Access request creation and decision use cases.

use crate::{
    application::context::ExecutionContext,
    domain::{access::AccessRequestStatus, permission::Capability},
};

use super::{
    AccessRequestView, AuthorizableEntry, DecideAccessRequestCommand, MetadataService,
    MetadataServiceError, MutationMetadata, RequestAccessByPathCommand, RequestAccessCommand,
    require_capability, validate_context,
};

impl MetadataService {
    /// Requests read or write access without disclosing hidden target metadata.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is unavailable, the requested access is already effective, or
    /// persistence fails.
    pub async fn request_access(
        &self,
        context: &ExecutionContext,
        command: &RequestAccessCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, command.entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        self.create_access_request(context, entry, command, metadata)
            .await
    }

    /// Requests access to the exact hidden entry named by a permanent URL path.
    ///
    /// This is the deliberate exception to normal path resolution: it reveals
    /// no entry metadata and returns the same access-request record as the UUID
    /// route. A missing path and a path in another tenant both remain opaque
    /// not-found results.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the path is unavailable, the requested access is already effective, or
    /// persistence fails.
    pub async fn request_access_by_path(
        &self,
        context: &ExecutionContext,
        command: &RequestAccessByPathCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry_by_path(context, &command.path)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        let command = RequestAccessCommand {
            entry_id: entry.entry.id,
            access: command.access,
            reason: command.reason.clone(),
        };
        self.create_access_request(context, entry, &command, metadata)
            .await
    }

    async fn create_access_request(
        &self,
        context: &ExecutionContext,
        entry: AuthorizableEntry,
        command: &RequestAccessCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView, MetadataServiceError> {
        let authorization = entry.authorization(context.authorization());
        // Requesting access the caller already has would create a request
        // nobody needs to decide. Effective access is entry-kind aware, so it
        // is the honest comparison for "would this add anything?".
        let effective = authorization.capabilities().effective_access();
        let already_satisfied = command
            .access
            .rights()
            .all(|right| effective.contains(&right.satisfied_by()));
        if already_satisfied {
            return Err(MetadataServiceError::Conflict);
        }
        self.repository
            .create_access_request(context, command, metadata)
            .await
            .map_err(Into::into)
    }

    /// Approves or denies a pending access request.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the request is unavailable or terminal, permission management is not
    /// authorized, or persistence fails.
    pub async fn decide_access_request(
        &self,
        context: &ExecutionContext,
        command: DecideAccessRequestCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView, MetadataServiceError> {
        validate_context(context)?;
        let request = self
            .repository
            .find_access_request(context, command.request_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        if request.request.status != AccessRequestStatus::Pending {
            return Err(MetadataServiceError::Conflict);
        }
        require_capability(&request.entry, context, Capability::ManagePermissions)?;
        self.repository
            .decide_access_request(context, command, metadata, Capability::ManagePermissions)
            .await
            .map_err(Into::into)
    }
}
