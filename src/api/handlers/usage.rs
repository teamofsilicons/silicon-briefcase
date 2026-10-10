//! Storage consumption and limit reporting.

use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
    http::HeaderMap,
};

use crate::error::AppError;

use super::super::{
    auth::Action,
    drives,
    dto::{DriveQuery, OrganizationUsageDto},
    extract,
    mapping::metadata_error,
    state::AppState,
    validation,
};

/// Returns a drive's consumption and limits, in bytes: the caller's own, or
/// (with `drive`) that of a Silicon it looks after.
pub(crate) async fn organization_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<DriveQuery>, QueryRejection>,
) -> Result<Json<OrganizationUsageDto>, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::drive(query.drive.as_deref()))?;
    let context = extract::authenticate(&state, &headers, Action::ReadUsage, "usage").await?;
    let context = drives::named(&state, context, query.drive.as_deref()).await?;
    let usage = extract::scoped(&context, state.metadata.organization_usage(&context))
        .await
        .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::usage(&usage)))
}
