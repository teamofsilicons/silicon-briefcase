//! Use cases for an application acting on behalf of a member.

use crate::{application::context::ExecutionContext, domain::permission::Capability};

use super::{
    AuthorizedEntryView, MetadataService, MetadataServiceError, require_capability,
    validate_context,
};

impl MetadataService {
    /// Returns the calling application's own folder for the represented actor.
    ///
    /// The folder is `private/{actor}/apps/{app_id}`, created on first use and
    /// reserved from then on.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request is not an application
    /// request, the folder cannot be materialized, or the represented actor
    /// may not create content inside it.
    pub async fn application_folder(
        &self,
        context: &ExecutionContext,
    ) -> Result<AuthorizedEntryView, MetadataServiceError> {
        validate_context(context)?;
        let folder = self.repository.ensure_application_folder(context).await?;
        let authorization = require_capability(&folder, context, Capability::CreateChild)?;
        folder.into_full_view(authorization)
    }
}
