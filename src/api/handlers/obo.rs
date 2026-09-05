//! The endpoint other applications may call on behalf of a member.
//!
//! Briefcase exposes exactly one OBO operation: create a file at any location
//! the represented member may write to, at any supported size. Everything that
//! decides where the file lands travels inside the proof as IAM-bound
//! metadata, never as a header or query parameter, so an application cannot
//! redirect a proof it legitimately obtained to a different destination.

use axum::{Json, body::Body, extract::State, http::HeaderMap, http::StatusCode};
use serde::Deserialize;

use crate::{
    application::{
        content::{StagedContent, UploadCommand},
        context::{ExecutionContext, TestingEnvironmentContext},
        idempotency::{IdempotencyKey, upload_fingerprint},
    },
    domain::{
        actor::RequestAuthContext,
        entry::{EntryName, EntryPath},
        ids::EntryId,
        multipart::MAX_UPLOAD_BYTES,
    },
    error::AppError,
    infrastructure::{iam::OboRequestBinding, testing::TestingEnvironmentAccess},
    request_context,
};

use super::super::{
    auth, dto::EntryDto, extract, mapping::metadata_error, state::AppState, upload,
};

/// Registered path of the file-creation endpoint, exactly as IAM stores it.
///
/// IAM signs and verifies this path rather than accepting one from a caller,
/// so it must stay byte-identical to the registered definition and to the
/// route the router serves.
pub(crate) const CREATE_FILE_PATH: &str = "/api/v1/obo/files";
/// Registered endpoint identifier of the file-creation endpoint.
pub(crate) const CREATE_FILE_ENDPOINT_ID: &str = "briefcase.files.create";

const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// Metadata an application binds into the proof when creating a file.
#[derive(Debug, Deserialize)]
struct CreateFileMetadata {
    /// Destination folder path; empty selects the application's own folder.
    #[serde(default)]
    path: String,
    /// File name to create.
    name: String,
    /// Media type of the bytes.
    #[serde(default)]
    content_type: String,
}

/// Creates a file for the represented member from a verified OBO proof.
pub(crate) async fn create_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<EntryDto>), AppError> {
    let (application, proof) = auth::obo_credentials(&headers)?;
    let declared_organization = auth::optional_organization(&headers)?;
    let testing_access = extract::optional_testing_access(&state, &headers).await?;
    let iam_environment = testing_access
        .as_ref()
        .map(extract::iam_environment_credential)
        .transpose()?;

    let file = upload::stage_body(body, state.temporary_directory.clone(), MAX_UPLOAD_BYTES)
        .await
        .map_err(extract::map_staging_error)?;
    let body_sha256 = hex::encode(file.sha256());
    // Do not hold the lifecycle fence while an untrusted caller streams its
    // body. Once staged, acquire it through single-use IAM proof verification
    // and exact activity touch. Repository transactions take their own shared
    // fence, avoiding a second simultaneous test-pool connection here.
    let fence = extract::testing_use_fence(&state, testing_access.as_ref()).await?;

    let verified = state
        .iam
        .verify_obo(
            &proof,
            &application,
            declared_organization.as_ref(),
            &OboRequestBinding {
                method: "POST",
                path: CREATE_FILE_PATH,
                body_sha256: &body_sha256,
            },
            iam_environment.as_ref(),
        )
        .await?;
    if verified.endpoint_id != CREATE_FILE_ENDPOINT_ID {
        return Err(AppError::Forbidden);
    }
    accept_testing_access(
        &state,
        testing_access.as_ref(),
        verified.organization_id.as_str(),
    )
    .await?;
    if let Some(fence) = fence {
        fence.release().await?;
    }

    let metadata: CreateFileMetadata = serde_json::from_value(verified.metadata.clone())
        .map_err(|_| AppError::validation("invalid_obo_metadata"))?;
    let name = EntryName::new(&metadata.name).map_err(|_| AppError::validation("invalid_name"))?;
    let content_type = normalized_content_type(&metadata.content_type)?;
    let destination = if metadata.path.trim().is_empty() {
        None
    } else {
        Some(EntryPath::new(&metadata.path).map_err(|_| AppError::validation("invalid_path"))?)
    };

    let request_id = request_context::current_request_id().ok_or(AppError::Internal {
        category: "request_scope_missing",
    })?;
    let authorization = verified.authorization.clone().ok_or(AppError::Forbidden)?;
    let context = represented_context(authorization, request_id, testing_access.as_ref());

    let parent_id = destination_folder(&state, &context, destination.as_ref()).await?;
    let resource = parent_id.to_string();
    let request_hash = upload_fingerprint(
        "obo_create_file",
        &resource,
        name.as_str(),
        &content_type,
        file.size(),
        file.sha256(),
    );
    // A proof is single-use, so its identifier is the natural idempotency key:
    // it cannot collide, and IAM has already refused any replay of it.
    let idempotency_key =
        IdempotencyKey::new(format!("obo-{}", verified.proof_id)).map_err(|_| {
            AppError::Internal {
                category: "obo_idempotency_key",
            }
        })?;
    let command = UploadCommand {
        parent_id,
        name,
        content_type,
        idempotency_key,
        request_hash,
    };
    let staged = StagedContent {
        path: file.path(),
        offset: 0,
        size: file.size(),
        sha256: *file.sha256(),
    };
    let entry_id =
        extract::scoped(&context, state.content.upload(&context, &command, staged)).await?;
    let entry = extract::scoped(&context, state.metadata.get_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok((StatusCode::CREATED, Json(state.mapper.entry(entry)?)))
}

async fn accept_testing_access(
    state: &AppState,
    access: Option<&TestingEnvironmentAccess>,
    verified_org_id: &str,
) -> Result<(), AppError> {
    if access.is_some_and(|access| access.owner_org_id != verified_org_id) {
        return Err(AppError::NotFound);
    }
    extract::touch_testing_access(state, access).await
}

fn represented_context(
    authorization: RequestAuthContext,
    request_id: String,
    access: Option<&TestingEnvironmentAccess>,
) -> ExecutionContext {
    match access {
        Some(access) => ExecutionContext::in_testing_environment(
            authorization,
            request_id,
            TestingEnvironmentContext::new(access.environment_id, access.control_version),
        ),
        None => ExecutionContext::new(authorization, request_id),
    }
}

/// Resolves the bound destination, defaulting to the application's own folder.
async fn destination_folder(
    state: &AppState,
    context: &ExecutionContext,
    destination: Option<&EntryPath>,
) -> Result<EntryId, AppError> {
    let Some(path) = destination else {
        let folder = extract::scoped(context, state.metadata.application_folder(context))
            .await
            .map_err(metadata_error)?;
        return Ok(folder.entry.id);
    };
    let parent = extract::scoped(context, state.metadata.get_entry_by_path(context, path))
        .await
        .map_err(metadata_error)?;
    if !parent.is_folder() {
        return Err(AppError::NotFound);
    }
    Ok(parent.id())
}

fn normalized_content_type(declared: &str) -> Result<String, AppError> {
    let declared = declared.trim();
    if declared.is_empty() {
        return Ok(DEFAULT_CONTENT_TYPE.to_owned());
    }
    if declared.len() > 255 || declared.parse::<mime::Mime>().is_err() {
        return Err(AppError::validation("invalid_content_type"));
    }
    Ok(declared.to_owned())
}
