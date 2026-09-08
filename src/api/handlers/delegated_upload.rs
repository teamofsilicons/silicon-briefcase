//! Fresh-proof upload control and narrowly capability-authorized byte transfer.

use axum::{
    Json,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse as _, Response},
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{
    super::{auth, extract, state::AppState, upload},
    delegated::{authorize_json, destination_folder, require_operation_id},
};
use crate::{
    application::{
        context::TestingEnvironmentContext,
        delegated_upload::{DelegatedUploadScope, DelegatedUploadStatus, ReserveDelegatedUpload},
    },
    domain::entry::EntryName,
    error::AppError,
    request_context,
};

pub(crate) const RESERVE_PATH: &str = "/api/v1/obo/uploads/reserve";
pub(crate) const COMMIT_PATH: &str = "/api/v1/obo/uploads/commit";
pub(crate) const STATUS_PATH: &str = "/api/v1/obo/uploads/status";
pub(crate) const CANCEL_PATH: &str = "/api/v1/obo/uploads/cancel";
pub(crate) const TRANSFER_PATH: &str = "/api/v1/obo/uploads/{upload_id}/content";
const CAPABILITY_HEADER: &str = "x-briefcase-upload-capability";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReserveRequest {
    operation_id: Uuid,
    parent_path: String,
    name: String,
    content_type: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    operation_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitRequest {
    operation_id: Uuid,
    upload_id: Uuid,
}

#[derive(Serialize)]
pub(crate) struct ReserveResponse {
    #[serde(flatten)]
    status: DelegatedUploadStatus,
    // Deliberately no Debug implementation: this is the only capability reply.
    capability: Option<String>,
}

pub(crate) async fn reserve(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let (context, body): (_, ReserveRequest) = authorize_json(
        &state,
        &headers,
        &body,
        RESERVE_PATH,
        "briefcase.uploads.reserve",
    )
    .await?;
    require_operation_id(body.operation_id)?;
    if body.content_type.len() > 255 || body.content_type.parse::<mime::Mime>().is_err() {
        return Err(AppError::validation("invalid_content_type"));
    }
    if body.sha256.len() != 64
        || !body
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::validation("invalid_sha256"));
    }
    let mut sha256 = [0_u8; 32];
    hex::decode_to_slice(&body.sha256, &mut sha256)
        .map_err(|_| AppError::validation("invalid_sha256"))?;
    let name = EntryName::new(&body.name).map_err(|_| AppError::validation("invalid_name"))?;
    let parent_id = destination_folder(&state, &context, &body.parent_path).await?;
    let manifest = serde_json::to_vec(&body).map_err(|_| AppError::Internal {
        category: "delegated_upload_manifest",
    })?;
    let command = ReserveDelegatedUpload {
        operation_id: body.operation_id,
        parent_path: body.parent_path,
        parent_id,
        name,
        content_type: body.content_type,
        size: body.size,
        sha256,
        request_hash: Sha256::digest(manifest).into(),
    };
    let reservation = extract::scoped(
        &context,
        state.delegated_uploads.reserve(&context, &command),
    )
    .await?;
    Ok((
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(ReserveResponse {
            status: reservation.status,
            capability: reservation
                .capability
                .map(|secret| secret.expose_secret().to_owned()),
        }),
    )
        .into_response())
}

pub(crate) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<DelegatedUploadStatus>, AppError> {
    let (context, body): (_, OperationRequest) = authorize_json(
        &state,
        &headers,
        &body,
        STATUS_PATH,
        "briefcase.uploads.status",
    )
    .await?;
    require_operation_id(body.operation_id)?;
    extract::scoped(
        &context,
        state.delegated_uploads.status(&context, body.operation_id),
    )
    .await
    .map(Json)
}

pub(crate) async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<DelegatedUploadStatus>, AppError> {
    let (context, body): (_, OperationRequest) = authorize_json(
        &state,
        &headers,
        &body,
        CANCEL_PATH,
        "briefcase.uploads.cancel",
    )
    .await?;
    require_operation_id(body.operation_id)?;
    extract::scoped(
        &context,
        state.delegated_uploads.cancel(&context, body.operation_id),
    )
    .await
    .map(Json)
}

pub(crate) async fn commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<DelegatedUploadStatus>, AppError> {
    let (context, body): (_, CommitRequest) = authorize_json(
        &state,
        &headers,
        &body,
        COMMIT_PATH,
        "briefcase.uploads.commit",
    )
    .await?;
    require_operation_id(body.operation_id)?;
    if body.upload_id.is_nil() {
        return Err(AppError::validation("invalid_upload_id"));
    }
    extract::scoped(
        &context,
        state
            .delegated_uploads
            .commit(&context, body.operation_id, body.upload_id),
    )
    .await
    .map(Json)
}

pub(crate) async fn transfer(
    State(state): State<AppState>,
    Path(upload_id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<DelegatedUploadStatus>, AppError> {
    // IAM credentials cannot broaden or substitute for this byte-only grant.
    if [
        header::AUTHORIZATION.as_str(),
        "x-iam-obo-access-proof",
        "x-app-id",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
    {
        return Err(AppError::bad_request("ambiguous_authentication"));
    }
    if headers.contains_key(header::TRANSFER_ENCODING)
        || headers.contains_key(header::CONTENT_ENCODING)
    {
        return Err(AppError::bad_request("invalid_upload_framing"));
    }
    let capability = SecretString::from(single_header(&headers, CAPABILITY_HEADER)?.to_owned());
    let length = single_header(&headers, header::CONTENT_LENGTH.as_str())?;
    if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::bad_request("invalid_content_length"));
    }
    let length: u64 = length
        .parse()
        .map_err(|_| AppError::bad_request("invalid_content_length"))?;
    let organization_id = auth::optional_organization(&headers)?
        .ok_or_else(|| AppError::bad_request("missing_org_id"))?;
    let testing_access = extract::optional_testing_access(&state, &headers).await?;
    if testing_access
        .as_ref()
        .is_some_and(|access| access.owner_org_id != organization_id.as_str())
    {
        return Err(AppError::NotFound);
    }
    let scope = DelegatedUploadScope {
        organization_id,
        testing_environment: testing_access.as_ref().map(|access| {
            TestingEnvironmentContext::new(access.environment_id, access.control_version)
        }),
        request_id: request_context::current_request_id().ok_or(AppError::Internal {
            category: "request_scope_missing",
        })?,
    };
    let lease = state
        .delegated_uploads
        .claim_transfer(&scope, upload_id, &capability)
        .await?;
    if length != lease.size() {
        return Err(AppError::validation("delegated_upload_manifest_mismatch"));
    }
    // The claim transaction owns its own fence; acquire this short activity
    // fence only afterwards, so concurrent requests never hold one test-pool
    // connection while waiting for another from the same bounded pool.
    let fence = extract::testing_use_fence(&state, testing_access.as_ref()).await?;
    extract::touch_testing_access(&state, testing_access.as_ref()).await?;
    if let Some(fence) = fence {
        fence.release().await?;
    }
    // No test-use fence survives this network stream. Each later database
    // phase and provider-mutation CAS revalidates the exact generation.
    let staged = upload::stage_body(body, state.temporary_directory.clone(), length)
        .await
        .map_err(extract::map_staging_error)?;
    state
        .delegated_uploads
        .store(lease, staged.path(), staged.size(), *staged.sha256())
        .await
        .map(Json)
}

fn single_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, AppError> {
    let mut values = headers.get_all(name).iter();
    let first = values
        .next()
        .ok_or_else(|| AppError::bad_request("missing_upload_header"))?;
    if values.next().is_some() {
        return Err(AppError::bad_request("duplicate_security_header"));
    }
    first
        .to_str()
        .map_err(|_| AppError::bad_request("invalid_upload_header"))
}
