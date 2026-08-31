//! Access request creation and decision use cases.

use crate::{
    application::context::ExecutionContext,
    domain::{
        access::AccessRequestStatus,
        permission::{AccessLevel, Capability},
    },
};

use super::{
    AccessRequestView, DecideAccessRequestCommand, MetadataService, MetadataServiceError,
    MutationMetadata, RequestAccessCommand, require_capability, validate_context,
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
        let authorization = entry.authorization(context.authorization());
        let already_satisfied = match command.access {
            AccessLevel::Read => authorization.allows(Capability::Read),
            AccessLevel::Write => authorization.allows(Capability::UpdateMetadata),
        };
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
