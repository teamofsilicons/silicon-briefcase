//! Recoverable-bin listing and subtree restoration.

use crate::{application::context::ExecutionContext, domain::permission::Capability};

use super::{
    AuthorizedEntryView, ListBinQuery, MetadataService, MetadataServiceError, MutationMetadata,
    Page, RestoreBinEntryCommand, require_capability, validate_context,
};

impl MetadataService {
    /// Lists recoverable entries eligible under owner/write/admin policy.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// the repository cannot list or audit the visible entries.
    pub async fn list_bin(
        &self,
        context: &ExecutionContext,
        query: &ListBinQuery,
    ) -> Result<Page<AuthorizedEntryView>, MetadataServiceError> {
        validate_context(context)?;
        let candidates = self.repository.list_bin_entries(context, query).await?;
        let mut accessed = Vec::with_capacity(candidates.items.len());
        let mut items = Vec::with_capacity(candidates.items.len());
        for entry in candidates.items {
            let Ok(authorization) = require_capability(&entry, context, Capability::Delete) else {
                continue;
            };
            accessed.push(entry.entry.id);
            items.push(entry.into_full_view(authorization)?);
        }
        if !accessed.is_empty() {
            self.repository
                .record_metadata_access(context, &accessed)
                .await?;
        }
        Ok(Page {
            items,
            next_cursor: candidates.next_cursor,
        })
    }

    /// Restores a recoverable subtree to its original or deterministic fallback parent.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the entry is unavailable, restoration is unauthorized, or persistence
    /// fails.
    pub async fn restore_bin_entry(
        &self,
        context: &ExecutionContext,
        command: RestoreBinEntryCommand,
        metadata: &MutationMetadata,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_bin_entry(context, command.entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        require_capability(&entry, context, Capability::UpdateMetadata)?;
        let restored = self
            .repository
            .restore_bin_entry(
                context,
                command.entry_id,
                metadata,
                Capability::UpdateMetadata,
            )
            .await?;
        let authorization = restored.authorization(context.authorization());
        restored.into_full_view(authorization)
    }
}
