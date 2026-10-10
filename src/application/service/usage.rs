//! Drive usage reporting.

use crate::{application::context::ExecutionContext, domain::quota::OrganizationUsage};

use super::{MetadataService, MetadataServiceError, validate_context};

impl MetadataService {
    /// Returns what the drive the request acts in consumes, in bytes, and the
    /// limits it consumes against.
    ///
    /// The figures belong to the drive's owner: they and the custodian of a
    /// Silicon that owns the drive may read them; anyone else is told the
    /// drive is not there.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid,
    /// the caller lacks the drive's owner rights, or repository access fails.
    pub async fn organization_usage(
        &self,
        context: &ExecutionContext,
    ) -> Result<OrganizationUsage, MetadataServiceError> {
        validate_context(context)?;
        if !context.authorization().has_owner_rights() {
            return Err(MetadataServiceError::NotFound);
        }
        self.repository
            .load_organization_usage(context)
            .await
            .map_err(Into::into)
    }
}
