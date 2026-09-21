//! Briefcase test data-plane discovery and legacy read compatibility.
use super::super::{extract, state::AppState};
use crate::{
    application::testing::TestingEnvironmentStatus, error::AppError,
    infrastructure::testing::TestingEnvironmentStore,
};
use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde::Deserialize;
use uuid::Uuid;
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListStatus {
    Active,
    Deleted,
    All,
}
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    status: Option<ListStatus>,
}
/// Environment management belongs to Honeycomb; never forward Briefcase user tokens.
pub(crate) async fn manage() -> Result<Response, AppError> {
    Err(AppError::conflict(
        "testing_environment_managed_by_honeycomb",
    ))
}
pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<crate::application::testing::TestingEnvironmentPage>, AppError> {
    let org_id = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let query = extract::query(query)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let status = match query.status.unwrap_or(ListStatus::Active) {
        ListStatus::Active => Some(TestingEnvironmentStatus::Active),
        ListStatus::Deleted => Some(TestingEnvironmentStatus::Deleted),
        ListStatus::All => None,
    };
    let page = testing_store(&state)?.list(&context, status).await?;
    Ok(Json(page))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let environment = testing_store(&state)?.get(&context, environment_id).await?;
    response_with_etag(StatusCode::OK, environment.version, environment, false)
}

pub(crate) async fn current(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let access = extract::testing_access(&state, &headers).await?;
    let _fence = extract::testing_use_fence(&state, Some(&access)).await?;
    extract::touch_testing_access(&state, Some(&access)).await?;
    let current = testing_store(&state)?.current(&access);
    Ok(private_json(StatusCode::OK, current))
}

fn testing_store(state: &AppState) -> Result<&TestingEnvironmentStore, AppError> {
    state
        .testing
        .as_deref()
        .ok_or(AppError::DependencyUnavailable {
            dependency: "testing_database",
        })
}

fn require_path_organization(headers: &HeaderMap, path_org_id: &str) -> Result<(), AppError> {
    if extract::organization_resource(headers)? == path_org_id {
        Ok(())
    } else {
        // A cross-tenant path is deliberately indistinguishable from an
        // absent environment.
        Err(AppError::NotFound)
    }
}

fn response_with_etag<T: serde::Serialize>(
    status: StatusCode,
    version: i64,
    value: T,
    secret: bool,
) -> Result<Response, AppError> {
    let etag =
        HeaderValue::from_str(&format!("\"{version}\"")).map_err(|_| AppError::Internal {
            category: "testing_environment_etag",
        })?;
    let mut response = (status, Json(value)).into_response();
    response.headers_mut().insert(header::ETAG, etag);
    if secret {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

fn private_json<T: serde::Serialize>(status: StatusCode, value: T) -> Response {
    (
        status,
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        )],
        Json(value),
    )
        .into_response()
}
