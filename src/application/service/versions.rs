//! Retained file-version listing and restore orchestration.

use crate::{
    application::context::ExecutionContext,
    domain::{entry::EntryKind, permission::Capability},
};

use super::{
    FileVersionView, ListVersionsQuery, MetadataService, MetadataServiceError, Page,
    require_capability, validate_context,
};

impl MetadataService {
    /// Lists retained versions using the current entry's read permission.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the target is unavailable, not a file, or unreadable, or repository
    /// listing/auditing fails.
    pub async fn list_versions(
        &self,
        context: &ExecutionContext,
        query: &ListVersionsQuery,
    ) -> Result<Page<FileVersionView>, MetadataServiceError> {
        validate_context(context)?;
        let entry = self
            .repository
            .find_active_entry(context, query.entry_id)
            .await?
            .ok_or(MetadataServiceError::NotFound)?;
        if entry.entry.kind != EntryKind::File {
            return Err(MetadataServiceError::Conflict);
        }
        require_capability(&entry, context, Capability::Read)?;
        let versions = self.repository.list_file_versions(context, query).await?;
        self.repository
            .record_metadata_access(context, &[query.entry_id])
            .await?;
        Ok(versions)
    }
}
