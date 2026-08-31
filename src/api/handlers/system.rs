//! Liveness, readiness, build identity, and IAM webhook routes.

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use tracing::info;

use crate::{application::webhook::WebhookApplyOutcome, error::AppError, infrastructure::postgres};

use super::super::{state::AppState, webhook};

#[derive(Serialize)]
pub(crate) struct StatusBody {
    status: &'static str,
}

#[derive(Serialize)]
pub(crate) struct VersionBody {
    service: &'static str,
    version: &'static str,
}

pub(crate) async fn health() -> Json<StatusBody> {
    Json(StatusBody { status: "ok" })
}

pub(crate) async fn ready(State(state): State<AppState>) -> Result<Json<StatusBody>, AppError> {
    if !postgres::ready(&state.database).await {
        return Err(AppError::DependencyUnavailable {
            dependency: "database",
        });
    }
    Ok(Json(StatusBody { status: "ready" }))
}

pub(crate) async fn version() -> Json<VersionBody> {
    Json(VersionBody {
        service: "silicon-briefcase",
        version: env!("CARGO_PKG_VERSION"),
    })
}

pub(crate) async fn iam_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<impl IntoResponse, AppError> {
    let body = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge
        } else {
            AppError::bad_request("invalid_webhook_body")
        }
    })?;
    let verified = webhook::verify(&headers, &body, &state.webhook_settings)?;
    let event_id = verified.event.event_id;
    let outcome = state.webhook_repository.apply_iam_event(&verified).await?;
    info!(%event_id, ?outcome, "IAM webhook processed");

    let status = match outcome {
        WebhookApplyOutcome::Applied
        | WebhookApplyOutcome::Duplicate
        | WebhookApplyOutcome::Stale
        | WebhookApplyOutcome::Ignored => StatusCode::NO_CONTENT,
    };
    Ok(status)
}
