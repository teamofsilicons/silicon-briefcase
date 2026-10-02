//! Retirement response for the former metadata-only raw OBO upload route.
//!
//! Current OBO uploads use reserve, capability transfer, and commit. Reject
//! legacy uploads before staging bytes or making any upstream IAM request.

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
};

use super::super::{dto::EntryDto, state::AppState};
use crate::error::AppError;

/// Stable retired route retained so existing callers receive migration guidance.
pub(crate) const CREATE_FILE_PATH: &str = "/api/v1/obo/files";

/// Rejects the retired raw upload without polling its request body.
pub(crate) async fn create_file(
    State(_state): State<AppState>,
    _headers: HeaderMap,
    _body: Body,
) -> Result<(StatusCode, Json<EntryDto>), AppError> {
    Err(AppError::RetiredOboUpload)
}
