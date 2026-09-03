//! Organization usage reporting.

use axum::{Json, extract::State, http::HeaderMap};

use crate::error::AppError;

use super::super::{
    auth::IamAction, dto::OrganizationUsageDto, extract, mapping::metadata_error, state::AppState,
};

/// Returns the organization's consumption and limits, in bytes.
pub(crate) async fn organization_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<OrganizationUsageDto>, AppError> {
    let organization = extract::organization_resource(&headers)?;
    let context =
        extract::authenticate(&state, &headers, IamAction::ReadUsage, &organization).await?;
    let usage = extract::scoped(&context, state.metadata.organization_usage(&context))
        .await
        .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::usage(&usage)))
}
