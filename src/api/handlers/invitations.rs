//! Invitations: sharing an entry with a Carbon or Silicon named by id.

use super::super::{
    auth::Action,
    drives, extract,
    mapping::{ResponseMapper, metadata_error},
    state::AppState,
};
use crate::domain::lifetime::LifetimeMinutes;
use crate::{
    application::{context::ExecutionContext, service::GrantPermissionCommand},
    domain::{
        actor::ActorKind,
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
use serde_json::Value;
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
    /// Makes this a read-only expiring share that ends after 1 to 43,200 minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_minutes: Option<LifetimeMinutes>,
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
    let context =
        extract::authenticate(&state, &headers, Action::GrantPermission, &id.to_string()).await?;
    let metadata = extract::mutation(&headers, "invite", &id.to_string(), &body, true)?;
    perform(&state, context, id, body, &metadata)
        .await
        .map(|value| (StatusCode::CREATED, Json(value)))
}

pub(super) async fn perform(
    state: &AppState,
    context: ExecutionContext,
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
    if body.expires_in_minutes.is_some()
        && body.access.iter().any(|right| *right != AccessRight::Read)
    {
        return Err(AppError::validation("expiring_share_is_read_only"));
    }
    let entry_id = extract::entry_id(id)?;
    let context = drives::for_entry(state, context, entry_id).await?;
    let access = GrantedAccess::new(body.access);
    let kind = match body.principal.kind.as_str() {
        "carbon" => ActorKind::Carbon,
        "silicon" => ActorKind::Silicon,
        "tag" => {
            return Err(AppError::unprocessable(
                "tag_recipients_retired",
                "Tags no longer exist in Briefcase. Share with each Carbon or Silicon by id (type carbon with c:…, or type silicon with si:…).",
            ));
        }
        "email" => {
            return Err(AppError::unprocessable(
                "email_recipients_retired",
                "Briefcase shares with accounts, not email addresses. Name the Carbon or Silicon by id (type carbon with c:…, or type silicon with si:…).",
            ));
        }
        _ => return Err(AppError::validation("invalid_principal_type")),
    };
    let principal = extract::resolve_recipient(state, &context, kind, &body.principal.id).await?;
    let context = extract::with_recipients(context, std::slice::from_ref(&principal));
    let grant = state
        .metadata
        .grant_permission(
            &context,
            &GrantPermissionCommand {
                entry_id,
                principal,
                access,
                inherits_to_descendants: body.inherit,
                lifetime: body.expires_in_minutes,
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
    let context =
        extract::authenticate(&state, &headers, Action::ListPermissions, &id.to_string()).await?;
    let context = drives::for_entry(&state, context, extract::entry_id(id)?).await?;
    let mut page = state
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
        .await?;
    // Keep the immutable authority key and add the current public label only
    // after permission to see this entry's recipients has been established.
    if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
        let keys: Vec<String> = items
            .iter()
            .filter_map(|item| item["principal"]["id"].as_str().map(str::to_owned))
            .collect();
        let labels = state.accounts.labels(&keys).await?;
        for item in items {
            if let Some(label) = item["principal"]["id"]
                .as_str()
                .and_then(|id| labels.get(id))
            {
                item["principal"]["public_id"] = Value::String(label.clone());
            }
        }
    }
    Ok(Json(page))
}

pub(crate) async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    let context =
        extract::authenticate(&state, &headers, Action::RevokePermission, &id.to_string()).await?;
    let context = drives::for_entry(&state, context, extract::entry_id(id)?).await?;
    let metadata = extract::mutation(&headers, "revoke_invitation", &id.to_string(), &grant, true)?;
    state
        .content_adapter
        .metadata_repository()
        .revoke_tag_or_member(&context, extract::entry_id(id)?, grant, &metadata)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changes one live expiring share. Exactly one field: a new lifetime counted
/// from now (extends or shortens it), or `permanent: true`.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpiringShareChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_minutes: Option<LifetimeMinutes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
}

impl ExpiringShareChange {
    /// The new lifetime, or `None` to make the share permanent.
    pub(crate) fn lifetime(&self) -> Result<Option<LifetimeMinutes>, AppError> {
        match (self.expires_in_minutes, self.permanent) {
            (Some(lifetime), None) => Ok(Some(lifetime)),
            (None, Some(true)) => Ok(None),
            _ => Err(AppError::validation("expires_in_minutes_or_permanent")),
        }
    }
}

pub(crate) async fn change(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
    Json(body): Json<ExpiringShareChange>,
) -> Result<Json<Value>, AppError> {
    let lifetime = body.lifetime()?;
    let context =
        extract::authenticate(&state, &headers, Action::GrantPermission, &id.to_string()).await?;
    let context = drives::for_entry(&state, context, extract::entry_id(id)?).await?;
    let metadata = extract::mutation(
        &headers,
        "change_expiring_share",
        &format!("{id}/{grant}"),
        &body,
        true,
    )?;
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .change_expiring_share(&context, extract::entry_id(id)?, grant, lifetime, &metadata)
            .await?,
    ))
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
    let (context, body): (_, DelegatedInvite) =
        super::delegated::authorize_json(&state, &headers, &body, "briefcase.invitations.create")
            .await?;
    let metadata = super::delegated::logical_mutation(
        "briefcase.invitations.create",
        &body.entry_id.to_string(),
        body.operation_id,
        &body,
    )?;
    Ok(Json(
        perform(&state, context, body.entry_id, body.invitation, &metadata).await?,
    ))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DelegatedLink {
    operation_id: Uuid,
    entry_id: Uuid,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_in_minutes: Option<LifetimeMinutes>,
}
pub(crate) async fn delegated_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<crate::api::dto::LinkAccessDto>, AppError> {
    let (context, body): (_, DelegatedLink) =
        super::delegated::authorize_json(&state, &headers, &body, "briefcase.link_access.update")
            .await?;
    let metadata = super::delegated::logical_mutation(
        "briefcase.link_access.update",
        &body.entry_id.to_string(),
        body.operation_id,
        &body,
    )?;
    let context = drives::for_entry(&state, context, extract::entry_id(body.entry_id)?).await?;
    let access = state
        .content_adapter
        .metadata_repository()
        .set_link_access(
            &context,
            extract::entry_id(body.entry_id)?,
            body.enabled,
            body.expires_in_minutes,
            &metadata,
        )
        .await?;
    let drive = context.authorization().organization_id().as_str();
    Ok(Json(
        state
            .mapper_for([drive])
            .await?
            .link_access(drive, &access)?,
    ))
}
