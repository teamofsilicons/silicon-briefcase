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
        context::ExecutionContext,
        idempotency::{IdempotencyKey, upload_fingerprint},
    },
    domain::{
        entry::{EntryName, EntryPath},
        ids::EntryId,
        multipart::MAX_UPLOAD_BYTES,
    },
    error::AppError,
    infrastructure::iam::OboRequestBinding,
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

    // The proof commits to the digest of the exact bytes, so they are staged
    // and hashed before IAM is asked to consume it.
    let file = upload::stage_body(body, state.temporary_directory.clone(), MAX_UPLOAD_BYTES)
        .await
        .map_err(extract::map_staging_error)?;
    let body_sha256 = hex::encode(file.sha256());

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
        )
        .await?;
    if verified.endpoint_id != CREATE_FILE_ENDPOINT_ID {
        return Err(AppError::Forbidden);
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
    let authorization = state
        .metadata
        .represented_authority(
            &verified.organization_id,
            &verified.actor,
            verified.issuer.clone(),
            &request_id,
        )
        .await
        .map_err(metadata_error)?;
    let context = ExecutionContext::new(authorization, request_id);

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
    // The size decides the storage route, so an application uploads a file of
    // any supported size through this one endpoint.
    let entry_id =
        extract::scoped(&context, state.content.upload(&context, &command, staged)).await?;
    let entry = extract::scoped(&context, state.metadata.get_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok((StatusCode::CREATED, Json(state.mapper.entry(entry)?)))
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
