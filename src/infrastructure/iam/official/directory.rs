//! Fresh, versioned recipients from IAM's read-only membership surface.

use serde::Deserialize;
use silicon_iam_client::Paging;

#[derive(Deserialize)]
struct MemberPage {
    items: Vec<DirectoryMember>,
    page: models::PageInfo,
}
#[derive(Deserialize)]
struct DirectoryMember {
    id: Uuid,
    org_id: String,
    principal: models::ActorRef,
    status: models::MembershipStatus,
    org_role: models::MembershipOrgRole,
    tags: Vec<models::TagSummary>,
    removed_at: Option<time::OffsetDateTime>,
    version: i64,
    authorization_epoch: i64,
}

use super::{
    ActorId, ActorKind, ActorRef, Credential, IamClient, IamClientError, IamEnvironmentCredential,
    Operation, OrganizationId, SecretString, Uuid, binding_mismatch, invalid_response, models,
    sdk_error,
};
use crate::domain::actor::{
    AuthenticationMode, IamMembershipBinding, OrganizationRole, RequestAuthContext, TagName,
};

impl IamClient {
    pub(crate) async fn resolve_directory_recipients(
        &self,
        token: &SecretString,
        caller: &RequestAuthContext,
        recipients: &[ActorRef],
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<Option<Vec<RequestAuthContext>>, IamClientError> {
        let binding = caller.iam_binding().ok_or(IamClientError::Rejected)?;
        let client = self
            .scoped_client(environment)?
            .with_credential(Credential::Bearer(token.clone()));
        let mut paging = Paging::new().limit(100);
        let mut found = Vec::new();
        let mut cursors = std::collections::BTreeSet::new();
        // Bounded work: never silently treat an incomplete directory as empty.
        for _ in 0..100 {
            let value = client
                .application_reads()
                .members(caller.organization_id().as_str(), &paging)
                .await
                .map_err(|error| sdk_error(error, Operation::Service))?;
            let page: MemberPage = serde_json::from_value(value)
                .map_err(|_| invalid_response("directory.membership_fields"))?;
            for member in page.items {
                if member.org_id != caller.organization_id().as_str() {
                    return Err(binding_mismatch("directory.organization"));
                }
                let kind = match member.principal.type_field {
                    models::ActorRefType::Carbon => ActorKind::Carbon,
                    models::ActorRefType::Silicon => ActorKind::Silicon,
                    _ => return Err(invalid_response("directory.actor_type")),
                };
                let actor = ActorRef::new(
                    kind,
                    ActorId::new(member.principal.public_id.clone())
                        .map_err(|_| invalid_response("directory.public_id"))?,
                );
                if recipients.is_empty() || recipients.contains(&actor) {
                    if found
                        .iter()
                        .any(|value: &RequestAuthContext| value.actor() == &actor)
                    {
                        return Err(invalid_response("directory.duplicate_member"));
                    }
                    found.push(directory_member(
                        member,
                        actor,
                        caller.organization_id(),
                        binding.organization_id,
                    )?);
                }
            }
            if !recipients.is_empty()
                && recipients
                    .iter()
                    .all(|actor| found.iter().any(|value| value.actor() == actor))
            {
                return Ok(Some(found));
            }
            if !page.page.has_more {
                return Ok(if recipients.is_empty() {
                    Some(found)
                } else {
                    None
                });
            }
            let cursor = page
                .page
                .next_cursor
                .ok_or_else(|| invalid_response("directory.cursor"))?;
            if !cursors.insert(cursor.clone()) {
                return Err(invalid_response("directory.repeated_cursor"));
            }
            paging = Paging::new().limit(100).after(cursor);
        }
        Err(invalid_response("directory.page_budget"))
    }
}

fn directory_member(
    member: DirectoryMember,
    actor: ActorRef,
    organization: &OrganizationId,
    organization_id: Uuid,
) -> Result<RequestAuthContext, IamClientError> {
    if member.status != models::MembershipStatus::Active
        || member.removed_at.is_some()
        || member.id.is_nil()
        || member.principal.principal_id.is_nil()
        || member.version < 1
        || member.authorization_epoch < 1
    {
        return Err(invalid_response("directory.membership"));
    }
    let role = match member.org_role {
        models::MembershipOrgRole::Owner => OrganizationRole::Owner,
        models::MembershipOrgRole::Admin => OrganizationRole::Admin,
        models::MembershipOrgRole::Member => OrganizationRole::Member,
        models::MembershipOrgRole::Other(_) => return Err(invalid_response("directory.role")),
    };
    let mut tags = Vec::new();
    for tag in member.tags {
        if tag.id.is_nil()
            || tags
                .iter()
                .any(|(id, name): &(Uuid, TagName)| *id == tag.id || name.as_str() == tag.name)
        {
            return Err(invalid_response("directory.tag"));
        }
        tags.push((
            tag.id,
            TagName::new(tag.name).map_err(|_| invalid_response("directory.tag_name"))?,
        ));
    }
    Ok(RequestAuthContext::new(
        organization.clone(),
        actor,
        role,
        tags.iter().map(|(_, name)| name.clone()),
        AuthenticationMode::Bearer,
    )
    .with_iam_binding(IamMembershipBinding {
        organization_id,
        principal_id: member.principal.principal_id,
        membership_id: member.id,
        membership_version: member.version,
        authorization_epoch: member.authorization_epoch,
        tags,
    }))
}
