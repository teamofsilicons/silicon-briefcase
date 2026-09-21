//! Authenticated service control plane, independent of IAM test sessions.
use crate::{
    api::state::AppState,
    error::AppError,
    infrastructure::testing::{TestingEnvironmentStore, honeycomb::HoneycombOperation},
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::Value;
use uuid::Uuid;

fn authenticate<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
) -> Result<&'a TestingEnvironmentStore, AppError> {
    let store = state.testing.as_deref().ok_or(AppError::Unauthenticated)?;
    if headers.get_all("authorization").iter().count() != 1 {
        return Err(AppError::Unauthenticated);
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthenticated)?;
    store.authenticate_honeycomb(token)?;
    Ok(store)
}
pub(crate) async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, id, operation)): Path<(String, Uuid, Uuid)>,
    Json(body): Json<HoneycombOperation>,
) -> Result<Json<Value>, AppError> {
    let store = authenticate(&state, &headers)?;
    if body.org_id != org || body.environment_id != id || body.operation_id != operation {
        return Err(AppError::validation("honeycomb_operation_path_mismatch"));
    }
    Ok(Json(store.honeycomb_operation(&body).await?))
}
pub(crate) async fn receipt(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, id, operation)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    let store = authenticate(&state, &headers)?;
    Ok(Json(store.honeycomb_receipt(&org, id, operation).await?))
}
