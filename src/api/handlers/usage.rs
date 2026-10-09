//! Storage consumption and limit reporting.

use axum::{Json, extract::State, http::HeaderMap};

use crate::error::AppError;

use super::super::{
    auth::Action, dto::OrganizationUsageDto, extract, mapping::metadata_error, state::AppState,
};

/// Returns the organization's consumption and limits, in bytes.
pub(crate) async fn organization_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<OrganizationUsageDto>, AppError> {
    let context = extract::authenticate(&state, &headers, Action::ReadUsage, "usage").await?;
    let usage = extract::scoped(&context, state.metadata.organization_usage(&context))
        .await
        .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::usage(&usage)))
}
