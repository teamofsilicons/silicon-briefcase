//! Browser access workflows; authorization remains in the Briefcase API.
use crate::{App, Result, bad, session};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use briefcase_client::{AccessDecision, AccessRight, IdempotencyKey, NewAccessRequest};
use serde::Deserialize;
use uuid::Uuid;

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestAccess {
    path: String,
    access: Vec<AccessRight>,
    reason: Option<String>,
    operation_id: Uuid,
}

pub(crate) async fn request(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<RequestAccess>,
) -> Result<Json<briefcase_client::AccessRequest>> {
    if input.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    let request = NewAccessRequest {
        access: input.access,
        reason: input.reason,
    };
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .request_access_by_path_with_key(
                &input.path,
                &request,
                &IdempotencyKey::new(input.operation_id.to_string())?,
            )
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Decision {
    Approve { access: Vec<AccessRight> },
    Deny,
}

pub(crate) async fn decide(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Decision>,
) -> Result<Json<briefcase_client::AccessRequest>> {
    let decision = match input {
        Decision::Approve { access } => AccessDecision::Approve(access),
        Decision::Deny => AccessDecision::Deny,
    };
    Ok(Json(
        session::client(&app, &headers)
            .await?
            .decide_access_request(id, &decision)
            .await?,
    ))
}
