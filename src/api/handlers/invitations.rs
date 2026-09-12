//! Invitations to current members, verified email contacts, and dynamic IAM tags.

use super::super::{
    auth::{self, IamAction},
    extract,
    mapping::{ResponseMapper, metadata_error},
    state::AppState,
};
use crate::{
    application::{context::ExecutionContext, service::GrantPermissionCommand},
    domain::{
        actor::{ActorId, ActorKind, ActorRef},
        permission::{AccessRight, GrantedAccess},
    },
    error::AppError,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipient {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvitationRequest {
    pub principal: Recipient,
    #[serde(default = "read_only")]
    pub access: Vec<AccessRight>,
    #[serde(default = "inherit")]
    pub inherit: bool,
}
fn read_only() -> Vec<AccessRight> {
    vec![AccessRight::Read]
}
const fn inherit() -> bool {
    true
}

pub(crate) async fn invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<InvitationRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        IamAction::GrantPermission,
        &id.to_string(),
    )
    .await?;
    let metadata = extract::mutation(&headers, "invite", &id.to_string(), &body, true)?;
    perform(&state, Some(&headers), context, id, body, &metadata)
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}

pub(super) async fn perform(
    state: &AppState,
    headers: Option<&HeaderMap>,
    mut context: ExecutionContext,
    id: Uuid,
    body: InvitationRequest,
    metadata: &crate::application::service::MutationMetadata,
) -> Result<Value, AppError> {
    if body.principal.id.is_empty() || body.principal.id.len() > 254 {
        return Err(AppError::validation("invalid_principal"));
    }
    if body.access.contains(&AccessRight::Delete) {
        return Err(AppError::validation("delete_cannot_be_shared"));
    }
    let entry_id = extract::entry_id(id)?;
    let access = GrantedAccess::new(body.access);
    let repo = state.content_adapter.metadata_repository();
    if body.principal.kind == "tag" {
        if let Some(headers) = headers {
            let testing = extract::optional_testing_access(state, headers).await?;
            let credential = testing
                .as_ref()
                .map(extract::iam_environment_credential)
                .transpose()?;
            let token = auth::parse_bearer(auth::require_bearer_only(headers)?)?;
            let members = state
                .iam
                .resolve_directory_recipients(
                    &token,
                    context.authorization(),
                    &[],
                    credential.as_ref(),
                )
                .await?
                .ok_or(AppError::NotFound)?;
            context = context.with_directory_members(members);
        }
        let grant_id = repo
            .invite_tag(
                &context,
                entry_id,
                &body.principal.id,
                access,
                body.inherit,
                metadata,
            )
            .await?;
        return Ok(
            json!({"id":grant_id,"principal":body.principal,"access":access.rights().collect::<Vec<_>>(),"inherit":body.inherit}),
        );
    }
    let principal = match body.principal.kind.as_str() {
        "email" => repo.member_by_email(&context, &body.principal.id).await?,
        "carbon" | "silicon" => ActorRef::new(
            if body.principal.kind == "carbon" {
                ActorKind::Carbon
            } else {
                ActorKind::Silicon
            },
            ActorId::new(body.principal.id)
                .map_err(|_| AppError::validation("invalid_principal"))?,
        ),
        _ => return Err(AppError::validation("invalid_principal_type")),
    };
    if let Some(headers) = headers {
        context = extract::with_directory_recipients(
            state,
            headers,
            context,
            std::slice::from_ref(&principal),
        )
        .await?;
    }
    let grant = state
        .metadata
        .grant_permission(
            &context,
            &GrantPermissionCommand {
                entry_id,
                principal,
                access,
                inherits_to_descendants: body.inherit,
            },
            metadata,
        )
        .await
        .map_err(metadata_error)?;
    serde_json::to_value(ResponseMapper::permission(&grant)).map_err(|_| AppError::Internal {
        category: "invitation_response",
    })
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<super::super::dto::PageQuery>,
) -> Result<Json<Value>, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        IamAction::ListPermissions,
        &id.to_string(),
    )
    .await?;
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .invitations(
                &context,
                extract::entry_id(id)?,
                query
                    .cursor
                    .map(|v| {
                        v.parse()
                            .map_err(|_| AppError::bad_request("invalid_cursor"))
                    })
                    .transpose()?,
            )
            .await?,
    ))
}

pub(crate) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        IamAction::RevokePermission,
        &id.to_string(),
    )
    .await?;
    let metadata = extract::mutation(&headers, "revoke_invitation", &id.to_string(), &grant, true)?;
    state
        .content_adapter
        .metadata_repository()
        .revoke_tag_or_member(&context, extract::entry_id(id)?, grant, &metadata)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DelegatedInvite {
    operation_id: Uuid,
    entry_id: Uuid,
    invitation: InvitationRequest,
}
pub(crate) const OBO_INVITE_PATH: &str = "/api/v1/obo/invitations";
pub(crate) const OBO_LINK_PATH: &str = "/api/v1/obo/link-access";

pub(crate) async fn delegated_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let (context, body): (_, DelegatedInvite) = super::delegated::authorize_json(
        &state,
        &headers,
        &body,
        OBO_INVITE_PATH,
        "briefcase.invitations.create",
    )
    .await?;
    let metadata = super::delegated::logical_mutation(
        "briefcase.invitations.create",
        &body.entry_id.to_string(),
        body.operation_id,
        &body,
    )?;
    Ok(Json(
        perform(
            &state,
            None,
            context,
            body.entry_id,
            body.invitation,
            &metadata,
        )
        .await?,
    ))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DelegatedLink {
    operation_id: Uuid,
    entry_id: Uuid,
    enabled: bool,
}
pub(crate) async fn delegated_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<crate::infrastructure::postgres::sharing::LinkAccess>, AppError> {
    let (context, body): (_, DelegatedLink) = super::delegated::authorize_json(
        &state,
        &headers,
        &body,
        OBO_LINK_PATH,
        "briefcase.link_access.update",
    )
    .await?;
    let metadata = super::delegated::logical_mutation(
        "briefcase.link_access.update",
        &body.entry_id.to_string(),
        body.operation_id,
        &body,
    )?;
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .set_link_access(
                &context,
                extract::entry_id(body.entry_id)?,
                body.enabled,
                &metadata,
            )
            .await?,
    ))
}
