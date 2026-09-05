//! Liveness, readiness, build identity, and IAM webhook routes.

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use tracing::info;

use crate::{
    application::{context::TestingEnvironmentContext, webhook::WebhookApplyOutcome},
    error::AppError,
    infrastructure::postgres,
};

use super::super::versioning;

use super::super::{state::AppState, webhook};

#[derive(Serialize)]
pub(crate) struct StatusBody {
    status: &'static str,
}

/// The compatibility document a client reads before its first real call.
#[derive(Serialize)]
pub(crate) struct VersionBody {
    service: &'static str,
    selected_api_version: &'static str,
    supported_api_versions: [&'static str; versioning::SUPPORTED_API_VERSIONS.len()],
    contract_version: &'static str,
    build: &'static str,
    operations: &'static [versioning::OperationVersion],
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
    if let Some(testing) = &state.testing
        && !postgres::ready(testing.test_pool()).await
    {
        return Err(AppError::DependencyUnavailable {
            dependency: "testing_database",
        });
    }
    Ok(Json(StatusBody { status: "ready" }))
}

/// Serves the compatibility document, negotiating the API major first.
///
/// The response is cacheable only per advertised client catalog, so `Vary`
/// names the request header the selection depends on. A client that supports
/// no served major is told so with `406` rather than being handed a document
/// for a contract it cannot speak.
pub(crate) async fn version(headers: HeaderMap) -> Result<impl IntoResponse, AppError> {
    let advertised = headers
        .get(versioning::SUPPORTED_VERSIONS_HEADER)
        .and_then(|value| value.to_str().ok());
    let selected = versioning::select(advertised).ok_or(AppError::UnsupportedApiVersion)?;
    let mut response = Json(VersionBody {
        service: versioning::SERVICE_NAME,
        selected_api_version: selected,
        supported_api_versions: versioning::SUPPORTED_API_VERSIONS,
        contract_version: versioning::CONTRACT_VERSION,
        build: env!("CARGO_PKG_VERSION"),
        operations: &versioning::OPERATIONS,
    })
    .into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(selected) {
        headers.insert(versioning::SELECTED_VERSION_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(versioning::SUPPORTED_VERSIONS_HEADER.as_str()) {
        headers.insert(http::header::VARY, value);
    }
    Ok(response)
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
    let testing_environment = if verified.is_testing() {
        let testing = state
            .testing
            .as_ref()
            .ok_or(AppError::DependencyUnavailable {
                dependency: "testing_database",
            })?;
        let matched = testing
            .resolve_iam_webhook(|candidate| verified.testing_key_matches(candidate))
            .await?;
        Some(matched.ok_or(AppError::Unauthenticated)?.0)
    } else {
        None
    };
    let event_id = verified.event.event_id;
    let outcome = state
        .webhook_repository
        .apply_iam_event(&verified, testing_environment)
        .await?;
    let testing_environment_id = testing_environment.map(TestingEnvironmentContext::id);
    info!(%event_id, ?testing_environment_id, ?outcome, "IAM webhook processed");

    let status = match outcome {
        WebhookApplyOutcome::Applied
        | WebhookApplyOutcome::Duplicate
        | WebhookApplyOutcome::Stale
        | WebhookApplyOutcome::Ignored => StatusCode::NO_CONTENT,
    };
    Ok(status)
}
