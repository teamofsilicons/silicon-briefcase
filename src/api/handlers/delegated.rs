//! Delegated folder, listing, content, and recoverable-trash calls.
//!
//! Every operation carries a Silicon Accounts User verification proof for the
//! route's scope. No destination, byte range, disposition, or mutation key
//! comes from a query or header, and the normal application services still
//! enforce the account's access.

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    application::{
        content::ContentIntent,
        context::ExecutionContext,
        idempotency::IdempotencyKey,
        service::{
            CONTENTS_PAGE_SIZE, CreateFolderCommand, ListEntriesQuery as ServiceListEntriesQuery,
            MutationMetadata, PageRequest,
        },
    },
    domain::{
        entry::{EntryName, EntryPath},
        filter::FilterQuery,
        ids::EntryId,
    },
    error::AppError,
};

use super::{
    super::{
        cursor,
        dto::{EntryDto, EntryPageDto, ListEntriesQuery},
        extract,
        mapping::metadata_error,
        state::AppState,
        validation,
    },
    content,
};

/// Registered fixed path for delegated folder creation.
pub(crate) const CREATE_FOLDER_PATH: &str = "/api/v1/obo/folders/create";
/// Proof scope for delegated folder creation.
pub(crate) const CREATE_FOLDER_ENDPOINT_ID: &str = crate::domain::delegation::FOLDERS_CREATE;
/// Registered fixed path for delegated entry listing.
pub(crate) const LIST_ENTRIES_PATH: &str = "/api/v1/obo/entries/list";
/// Proof scope for delegated entry listing.
pub(crate) const LIST_ENTRIES_ENDPOINT_ID: &str = crate::domain::delegation::ENTRIES_LIST;
/// Registered fixed path for delegated file delivery.
pub(crate) const READ_FILE_PATH: &str = "/api/v1/obo/files/read";
/// Proof scope for delegated file delivery.
pub(crate) const READ_FILE_ENDPOINT_ID: &str = crate::domain::delegation::FILES_READ;
/// Registered fixed path for delegated recoverable deletion.
pub(crate) const TRASH_ENTRY_PATH: &str = "/api/v1/obo/entries/trash";
/// Proof scope for delegated recoverable deletion.
pub(crate) const TRASH_ENTRY_ENDPOINT_ID: &str = crate::domain::delegation::ENTRIES_TRASH;

/// JSON for a logical folder creation.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateFolderRequest {
    /// Stable caller-generated UUID, reused with fresh proofs on retry.
    pub(crate) operation_id: Uuid,
    /// Existing parent path; the empty string selects the private app folder.
    pub(crate) parent_path: String,
    /// One child folder name, not a path or root declaration.
    pub(crate) name: String,
}

/// JSON for one current file read.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadFileRequest {
    /// The exact file identifier within the authenticated organization.
    pub(crate) entry_id: Uuid,
    /// Optional HTTP byte-range syntax, bound inside the proof's JSON digest.
    pub(crate) range: Option<String>,
    /// Whether to return an attachment instead of sandboxed inline content.
    #[serde(default)]
    pub(crate) download: bool,
}

/// JSON for a logical recoverable deletion.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrashEntryRequest {
    /// Stable caller-generated UUID, reused with fresh proofs on retry.
    pub(crate) operation_id: Uuid,
    /// The exact entry to move to the recoverable bin.
    pub(crate) entry_id: Uuid,
}

/// Authenticates an app acting for an account and parses its JSON request.
///
/// The app must send `Authorization: Proof sap_…`: a Silicon Accounts User
/// verification proof whose receiving app is Briefcase, carrying `scope` (the
/// route's endpoint id), from an app allowed for that scope. The request then
/// runs as the account the proof speaks for, confined to `apps/{issuing app}/`.
/// The route must apply the normal JSON body limit before extracting `Bytes`.
pub(super) async fn authorize_json<T: DeserializeOwned>(
    state: &AppState,
    headers: &HeaderMap,
    body: &Bytes,
    scope: &'static str,
) -> Result<(ExecutionContext, T), AppError> {
    let context = extract::authenticate_proof(state, headers, scope).await?;
    let parsed = serde_json::from_slice(body).map_err(|_| AppError::bad_request("invalid_json"))?;
    Ok((context, parsed))
}

/// Creates a child folder with the represented member's ordinary write access.
pub(crate) async fn create_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<EntryDto>), AppError> {
    let (context, body): (_, CreateFolderRequest) =
        authorize_json(&state, &headers, &body, CREATE_FOLDER_ENDPOINT_ID).await?;
    require_operation_id(body.operation_id)?;
    let name = EntryName::new(&body.name).map_err(|_| AppError::validation("invalid_name"))?;
    let parent_id = destination_folder(&state, &context, &body.parent_path).await?;
    let metadata = logical_mutation(
        CREATE_FOLDER_ENDPOINT_ID,
        &parent_id.to_string(),
        body.operation_id,
        &body,
    )?;
    let command = CreateFolderCommand::new(name, Some(parent_id), None, Vec::new())
        .map_err(|_| AppError::validation("invalid_folder"))?;
    let created = extract::scoped(
        &context,
        state.metadata.create_folder(&context, command, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok((StatusCode::CREATED, Json(state.mapper.entry(created)?)))
}

/// Lists the same privacy-filtered view as the ordinary member endpoint.
pub(crate) async fn list_entries(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<EntryPageDto>, AppError> {
    let (context, query): (_, ListEntriesQuery) =
        authorize_json(&state, &headers, &body, LIST_ENTRIES_ENDPOINT_ID).await?;
    extract::validation(validation::list_entries(&query))?;
    if let Some(cursor) = query.cursor.as_deref() {
        cursor::validate_opaque(cursor).map_err(|_| AppError::bad_request("invalid_cursor"))?;
    }
    let filter = query
        .filter
        .as_deref()
        .map(FilterQuery::parse)
        .transpose()
        .map_err(|_| AppError::validation("invalid_filter"))?;
    let parent_id = match (query.parent_id, query.path.as_deref()) {
        (Some(_), Some(_)) => return Err(AppError::bad_request("ambiguous_parent")),
        (Some(id), None) => Some(extract::entry_id(id)?),
        (None, Some(path)) => Some(existing_folder(&state, &context, path).await?),
        (None, None) => {
            let folder = state
                .metadata
                .application_folder(&context)
                .await
                .map_err(metadata_error)?;
            Some(folder.entry.id)
        }
    };
    let page = PageRequest::new(query.cursor, query.limit.unwrap_or(CONTENTS_PAGE_SIZE))
        .map_err(|_| AppError::validation("invalid_pagination"))?;
    let result = extract::scoped(
        &context,
        state.metadata.list_entries(
            &context,
            &ServiceListEntriesQuery {
                parent_id,
                filter,
                page,
            },
        ),
    )
    .await
    .map_err(metadata_error)?;
    let organization = context.authorization().organization_id().as_str();
    let items = result
        .items
        .into_iter()
        .map(|item| state.mapper.entry_item(organization, item))
        .collect::<Result<_, _>>()?;
    Ok(Json(EntryPageDto {
        items,
        next_cursor: result.next_cursor,
    }))
}

/// Streams a file using only the range and disposition bound in the JSON body.
pub(crate) async fn read_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let (context, body): (_, ReadFileRequest) =
        authorize_json(&state, &headers, &body, READ_FILE_ENDPOINT_ID).await?;
    let entry_id = extract::entry_id(body.entry_id)?;
    let intent = if body.download {
        ContentIntent::Download
    } else {
        ContentIntent::Render
    };
    let mut bound_headers = HeaderMap::new();
    if let Some(range) = body.range {
        let value =
            HeaderValue::from_str(&range).map_err(|_| AppError::bad_request("invalid_range"))?;
        bound_headers.insert(header::RANGE, value);
    }
    content::serve(&state, &bound_headers, &context, entry_id, intent).await
}

/// Moves an entry to the recoverable bin using ordinary subtree-delete checks.
pub(crate) async fn trash_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, AppError> {
    let (context, body): (_, TrashEntryRequest) =
        authorize_json(&state, &headers, &body, TRASH_ENTRY_ENDPOINT_ID).await?;
    let entry_id = extract::entry_id(body.entry_id)?;
    let metadata = logical_mutation(
        TRASH_ENTRY_ENDPOINT_ID,
        &entry_id.to_string(),
        body.operation_id,
        &body,
    )?;
    extract::scoped(
        &context,
        state
            .metadata
            .soft_delete_entry(&context, entry_id, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn destination_folder(
    state: &AppState,
    context: &ExecutionContext,
    path: &str,
) -> Result<EntryId, AppError> {
    state
        .metadata
        .application_folder(context)
        .await
        .map_err(metadata_error)?;
    if path.is_empty() {
        let folder = extract::scoped(context, state.metadata.application_folder(context))
            .await
            .map_err(metadata_error)?;
        Ok(folder.entry.id)
    } else {
        existing_folder(state, context, path).await
    }
}

async fn existing_folder(
    state: &AppState,
    context: &ExecutionContext,
    path: &str,
) -> Result<EntryId, AppError> {
    let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
    let parent = extract::scoped(context, state.metadata.get_entry_by_path(context, &path))
        .await
        .map_err(metadata_error)?;
    if !parent.is_folder() {
        return Err(AppError::NotFound);
    }
    Ok(parent.id())
}

pub(super) fn logical_mutation<T: Serialize>(
    operation: &'static str,
    resource: &str,
    operation_id: Uuid,
    body: &T,
) -> Result<MutationMetadata, AppError> {
    require_operation_id(operation_id)?;
    // The repository also scopes this key by organization, represented actor,
    // originating application, and operation. Fresh proofs share this stable
    // logical key, but never their consumed proof or authorization snapshot.
    let key = IdempotencyKey::new(format!("delegated-{operation_id}"))
        .map_err(|_| AppError::validation("invalid_operation_id"))?;
    let fingerprint = extract::request_fingerprint(operation, resource, body)?;
    Ok(MutationMetadata::new(Some(key), fingerprint))
}

pub(super) fn require_operation_id(operation_id: Uuid) -> Result<(), AppError> {
    if operation_id.is_nil() {
        Err(AppError::validation("invalid_operation_id"))
    } else {
        Ok(())
    }
}
