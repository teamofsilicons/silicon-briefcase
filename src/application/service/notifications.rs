//! Notification inbox use cases.

use crate::{application::context::ExecutionContext, domain::notification::NotificationInbox};

use super::{MetadataService, MetadataServiceError, MutationMetadata, validate_context};

impl MetadataService {
    /// Returns the caller's newest notifications and unread badge count.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// repository access fails.
    pub async fn notification_inbox(
        &self,
        context: &ExecutionContext,
    ) -> Result<NotificationInbox, MetadataServiceError> {
        validate_context(context)?;
        self.repository
            .load_notification_inbox(context)
            .await
            .map_err(Into::into)
    }

    /// Marks the caller's entire inbox read and returns it afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the request context is invalid or
    /// persistence fails.
    pub async fn mark_notifications_read(
        &self,
        context: &ExecutionContext,
        metadata: &MutationMetadata,
    ) -> Result<NotificationInbox, MetadataServiceError> {
        validate_context(context)?;
        self.repository
            .mark_notifications_read(context, metadata)
            .await
            .map_err(Into::into)
    }
}
