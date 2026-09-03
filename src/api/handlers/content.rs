//! Streaming content, multipart, delivery, version, and storage handlers.

use axum::{
    Json,
    extract::{
        Multipart, Path, State,
        multipart::{Field, MultipartRejection},
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::Response,
};
use uuid::Uuid;

use crate::{
    application::{
        content::{
            ConfigureStorageCommand, ContentIntent, RestoreVersionCommand, StagedContent,
            UploadCommand,
        },
        context::ExecutionContext,
        idempotency::upload_fingerprint,
        service::{ListVersionsQuery, PageRequest},
    },
    domain::{
        entry::{EntryName, EntryPath},
        ids::EntryId,
        multipart::MAX_UPLOAD_BYTES,
        storage::EncryptionMode,
    },
    error::AppError,
};

use super::super::{
    auth::{self, IamAction},
    delivery,
    dto::{
        BucketConfigurationDto, BucketConfigurationStateDto, BucketConfigurationStatusDto,
        EncryptionModeDto, EntryDto, FileVersionPageDto,
    },
    extract,
    mapping::metadata_error,
    state::AppState,
    upload::{self, StagedUpload},
    validation,
};

const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// Streams current file bytes for in-place rendering.
pub(crate) async fn read_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    serve_entry_content(&state, &headers, entry_id, ContentIntent::Render).await
}

/// Streams current file bytes as a local download.
pub(crate) async fn download_content(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    serve_entry_content(&state, &headers, entry_id, ContentIntent::Download).await
}

async fn serve_entry_content(
    state: &AppState,
    headers: &HeaderMap,
    entry_id: EntryId,
    intent: ContentIntent,
) -> Result<Response, AppError> {
    let resource = entry_id.to_string();
    let action = match intent {
        ContentIntent::Render => IamAction::ReadContent,
        ContentIntent::Download => IamAction::DownloadFile,
    };
    let context = extract::authenticate(state, headers, action, &resource).await?;
    serve(state, headers, &context, entry_id, intent).await
}

/// Streams already-authorized file bytes with the sandboxed response headers.
pub(crate) async fn serve(
    state: &AppState,
    headers: &HeaderMap,
    context: &ExecutionContext,
    entry_id: EntryId,
    intent: ContentIntent,
) -> Result<Response, AppError> {
    let range = delivery::requested_range(headers)?;
    let delivery = extract::scoped(
        context,
        state.content.open_content(context, entry_id, intent, range),
    )
    .await?;
    delivery::response(delivery, intent)
}

/// Stores an uploaded file of any supported size.
///
/// The client streams one request; Briefcase decides internally whether the
/// bytes fit a single provider request or need a multipart transfer. Uploading
/// over an existing file publishes that file's next version.
pub(crate) async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<EntryDto>), AppError> {
    // Verify the credential shape before admitting a single byte of body.
    auth::require_bearer_shape(&headers)?;
    let idempotency_key = extract::required_idempotency_key(&headers)?;
    let organization = extract::organization_resource(&headers)?;
    let context =
        extract::authenticate(&state, &headers, IamAction::UploadFile, &organization).await?;
    let parts = parse_upload(
        extract::multipart(multipart)?,
        state.temporary_directory.clone(),
    )
    .await?;
    let parent_id = destination_folder(&state, &context, &parts.destination).await?;
    let resource = parent_id.to_string();
    let request_hash = upload_fingerprint(
        "upload_file",
        &resource,
        parts.name.as_str(),
        &parts.content_type,
        parts.file.size(),
        parts.file.sha256(),
    );
    let command = UploadCommand {
        parent_id,
        name: parts.name,
        content_type: parts.content_type,
        idempotency_key,
        request_hash,
    };
    let staged = StagedContent {
        path: parts.file.path(),
        offset: 0,
        size: parts.file.size(),
        sha256: *parts.file.sha256(),
    };
    let entry_id =
        extract::scoped(&context, state.content.upload(&context, &command, staged)).await?;
    let entry = extract::scoped(&context, state.metadata.get_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok((StatusCode::CREATED, Json(state.mapper.entry(entry)?)))
}

pub(crate) async fn list_versions(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<FileVersionPageDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::ListVersions, &resource).await?;
    let page = extract::scoped(
        &context,
        state.metadata.list_versions(
            &context,
            &ListVersionsQuery {
                entry_id,
                page: PageRequest::new(None, 50).map_err(|_| AppError::Internal {
                    category: "version_page_limit",
                })?,
            },
        ),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(FileVersionPageDto {
        items: super::super::mapping::ResponseMapper::versions(page.items)?,
    }))
}

pub(crate) async fn restore_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(Uuid, String)>, PathRejection>,
) -> Result<Json<EntryDto>, AppError> {
    let (entry_uuid, version_text) = extract::path(path)?;
    let entry_id = extract::entry_id(entry_uuid)?;
    let version_id = extract::version_id(&version_text)?;
    let resource = version_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::RestoreVersion, &resource).await?;
    let idempotency_key = extract::required_idempotency_key(&headers)?;
    let request_hash =
        extract::request_fingerprint("restore_version", &resource, &(entry_uuid, &version_text))?;
    let restored_entry_id = extract::scoped(
        &context,
        state.content.restore_version(
            &context,
            &RestoreVersionCommand {
                entry_id,
                version_id,
                idempotency_key,
                request_hash,
            },
        ),
    )
    .await?;
    let entry = extract::scoped(
        &context,
        state.metadata.get_entry(&context, restored_entry_id),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry(entry)?))
}

pub(crate) async fn configure_storage(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<BucketConfigurationDto>, JsonRejection>,
) -> Result<Json<BucketConfigurationStatusDto>, AppError> {
    let body = extract::json(body)?;
    extract::validation(validation::bucket_configuration(&body))?;
    let context =
        extract::authenticate_bearer(&state, &headers, IamAction::ConfigureStorage).await?;
    let resource = context.authorization().organization_id().as_str();
    let idempotency = extract::idempotency_key(&headers)?
        .map(|key| {
            extract::request_fingerprint("configure_storage", resource, &body)
                .map(|fingerprint| (key, fingerprint))
        })
        .transpose()?;
    let command = ConfigureStorageCommand {
        bucket_name: body.bucket_name,
        region: body.region,
        role_arn: body.role_arn,
        prefix: body.prefix,
        aws_account_id: body.aws_account_id,
        encryption: match body.encryption_mode {
            EncryptionModeDto::SseS3 => EncryptionMode::SseS3,
            EncryptionModeDto::SseKms => EncryptionMode::SseKms,
        },
        kms_key_arn: body.kms_key_arn,
        idempotency,
    };
    let result = extract::scoped(
        &context,
        state.content.configure_storage(&context, &command),
    )
    .await?;
    Ok(Json(BucketConfigurationStatusDto {
        status: if result.configured {
            BucketConfigurationStateDto::Configured
        } else {
            BucketConfigurationStateDto::Failed
        },
        tested_at: result.tested_at,
        failure_reason: result.failure_reason,
    }))
}

/// Where an upload should land, named either way the contract allows.
enum UploadDestination {
    /// Destination folder identifier.
    Folder(EntryId),
    /// Destination folder path, resolved from the organization base.
    Path(EntryPath),
}

struct UploadParts {
    destination: UploadDestination,
    name: EntryName,
    content_type: String,
    file: StagedUpload,
}

/// Resolves the destination folder, whichever way the client addressed it.
async fn destination_folder(
    state: &AppState,
    context: &ExecutionContext,
    destination: &UploadDestination,
) -> Result<EntryId, AppError> {
    match destination {
        UploadDestination::Folder(entry_id) => Ok(*entry_id),
        UploadDestination::Path(path) => {
            let parent = extract::scoped(context, state.metadata.get_entry_by_path(context, path))
                .await
                .map_err(metadata_error)?;
            if !parent.is_folder() {
                return Err(AppError::NotFound);
            }
            Ok(parent.id())
        }
    }
}

async fn parse_upload(
    mut multipart: Multipart,
    temporary_directory: std::path::PathBuf,
) -> Result<UploadParts, AppError> {
    let mut destination = None;
    let mut file = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| extract::map_multipart_error(&error))?
    {
        let field_name = field
            .name()
            .ok_or_else(|| AppError::bad_request("unnamed_multipart_field"))?
            .to_owned();
        match field_name.as_str() {
            "parent_id" => {
                if destination.is_some() {
                    return Err(AppError::bad_request("duplicate_destination"));
                }
                let value = read_text_field(field, 64).await?;
                let value = Uuid::parse_str(value.trim())
                    .map_err(|_| AppError::bad_request("invalid_parent_id"))?;
                destination = Some(UploadDestination::Folder(extract::entry_id(value)?));
            }
            "path" => {
                if destination.is_some() {
                    return Err(AppError::bad_request("duplicate_destination"));
                }
                let value = read_text_field(field, 2_048).await?;
                let path =
                    EntryPath::new(&value).map_err(|_| AppError::validation("invalid_path"))?;
                destination = Some(UploadDestination::Path(path));
            }
            "file" => {
                if file.is_some() {
                    return Err(AppError::bad_request("duplicate_file"));
                }
                let name = field
                    .file_name()
                    .ok_or_else(|| AppError::bad_request("missing_filename"))?
                    .to_owned();
                let content_type = field
                    .content_type()
                    .map_or_else(|| DEFAULT_CONTENT_TYPE.to_owned(), ToString::to_string);
                if content_type.len() > 255 || content_type.parse::<mime::Mime>().is_err() {
                    return Err(AppError::validation("invalid_content_type"));
                }
                let name =
                    EntryName::new(name).map_err(|_| AppError::validation("invalid_name"))?;
                let staged = upload::stage_multipart_field(
                    field,
                    temporary_directory.clone(),
                    MAX_UPLOAD_BYTES,
                )
                .await
                .map_err(extract::map_staging_error)?;
                file = Some((name, content_type, staged));
            }
            _ => return Err(AppError::bad_request("unknown_multipart_field")),
        }
    }

    let destination = destination.ok_or_else(|| AppError::bad_request("missing_destination"))?;
    let (name, content_type, file) = file.ok_or_else(|| AppError::bad_request("missing_file"))?;
    Ok(UploadParts {
        destination,
        name,
        content_type,
        file,
    })
}

async fn read_text_field(mut field: Field<'_>, maximum_bytes: usize) -> Result<String, AppError> {
    let mut value = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|error| extract::map_multipart_error(&error))?
    {
        let new_length = value
            .len()
            .checked_add(chunk.len())
            .ok_or(AppError::PayloadTooLarge)?;
        if new_length > maximum_bytes {
            return Err(AppError::PayloadTooLarge);
        }
        value.extend_from_slice(&chunk);
    }
    String::from_utf8(value).map_err(|_| AppError::bad_request("invalid_multipart_text"))
}
