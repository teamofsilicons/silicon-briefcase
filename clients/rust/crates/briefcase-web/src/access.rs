//! Browser access workflows; authorization remains in the Briefcase API.
use crate::{App, Result, session};
use axum::extract::{Path, Query};
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct Page {
    pub cursor: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct LinkUpdate {
    enabled: bool,
    operation_id: Uuid,
}
#[derive(Deserialize)]
pub(crate) struct InvitationInput {
    #[serde(flatten)]
    invite: briefcase_client::Invite,
    operation_id: Uuid,
}
#[derive(Deserialize)]
pub(crate) struct Mutation {
    operation_id: Uuid,
}
fn key(id: Uuid) -> Result<briefcase_client::IdempotencyKey> {
    if id.is_nil() {
        return Err(crate::bad("Missing operation identity"));
    }
    Ok(briefcase_client::IdempotencyKey::new(id.to_string())?)
}
pub(crate) async fn link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<briefcase_client::LinkAccess>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .link_access(id)
            .await?,
    ))
}
pub(crate) async fn set_link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<LinkUpdate>,
) -> Result<Json<briefcase_client::LinkAccess>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .set_link_access(id, input.enabled, &key(input.operation_id)?)
            .await?,
    ))
}
pub(crate) async fn logs(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<briefcase_client::LogPage>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .logs(id, page.cursor.as_deref())
            .await?,
    ))
}
pub(crate) async fn invitations(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<briefcase_client::InvitationPage>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .invitations(id, page.cursor.as_deref())
            .await?,
    ))
}
pub(crate) async fn invite(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<InvitationInput>,
) -> Result<Json<briefcase_client::Invitation>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .invite(id, &input.invite, &key(input.operation_id)?)
            .await?,
    ))
}
pub(crate) async fn revoke(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
    Json(input): Json<Mutation>,
) -> Result<Json<serde_json::Value>> {
    session::client(&app, &headers)
        .await?
        .revoke_invitation(id, grant, &key(input.operation_id)?)
        .await?;
    Ok(Json(serde_json::json!({"revoked":true})))
}

pub(crate) async fn inbox(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<briefcase_client::NotificationInbox>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .notifications()
            .await?,
    ))
}

pub(crate) async fn mark_read(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<briefcase_client::NotificationInbox>> {
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .mark_notifications_read()
            .await?,
    ))
}
