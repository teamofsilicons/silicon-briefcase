//! Use cases for an application acting on behalf of a member.

use crate::{
    application::context::ExecutionContext,
    domain::{
        actor::{
            ActorRef, ApplicationId, AuthenticationMode, OrganizationId, OrganizationRole,
            RequestAuthContext,
        },
        permission::Capability,
    },
};

use super::{
    AuthorizedEntryView, MetadataService, MetadataServiceError, require_capability,
    validate_context,
};

impl MetadataService {
    /// Builds request authority for an application acting for one member.
    ///
    /// IAM has already proven the represented actor and the organization. Role
    /// and tags are not part of that proof, so they come from Briefcase's IAM
    /// projection — and when the projection has not caught up yet, the request
    /// runs with the least authority any member has: no tags and no
    /// administrative access. That can only ever under-grant.
    ///
    /// # Errors
    ///
    /// Returns [`MetadataServiceError`] when the projection cannot be read.
    pub async fn represented_authority(
        &self,
        organization_id: &OrganizationId,
        actor: &ActorRef,
        application_id: ApplicationId,
        request_id: &str,
    ) -> Result<RequestAuthContext, MetadataServiceError> {
        let projected = self
            .repository
            .project_member_authorization(organization_id, actor, request_id)
            .await?;
        let (role, tags) = projected.map_or_else(
            || (OrganizationRole::Member, Vec::new()),
            |membership| (membership.role, membership.tags.into_iter().collect()),
        );
        Ok(RequestAuthContext::new(
            organization_id.clone(),
            actor.clone(),
            role,
            tags,
            AuthenticationMode::OnBehalfOf { application_id },
        ))
    }

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
