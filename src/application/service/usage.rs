//! Organization usage reporting.

use crate::{application::context::ExecutionContext, domain::quota::OrganizationUsage};

use super::{MetadataService, MetadataServiceError, validate_context};

impl MetadataService {
    /// Returns what the organization currently consumes, in bytes.
    ///
    /// The figures are the organization's own, not the caller's: any current
    /// member may ask how much space their organization is using and how much
    /// it may use.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// repository access fails.
    pub async fn organization_usage(
        &self,
        context: &ExecutionContext,
    ) -> Result<OrganizationUsage, MetadataServiceError> {
        validate_context(context)?;
        self.repository
            .load_organization_usage(context)
            .await
            .map_err(Into::into)
    }
}
