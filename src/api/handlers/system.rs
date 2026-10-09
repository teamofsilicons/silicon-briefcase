//! Liveness, readiness and build identity routes.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
};
use serde::Serialize;

use crate::{error::AppError, infrastructure::postgres};

use super::super::{state::AppState, versioning};

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
