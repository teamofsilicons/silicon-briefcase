//! Permanent addresses.
//!
//! An entry's permanent address names its drive by the owner's id and
//! continues with the entry's path: `https://briefcase.teamofsilicons.com/c:ada/notes/plan.md`.
//! The API resolves it at `GET /api/v1/paths/{owner}/{path}` (the owner as a
//! `c:`/`si:` id or as the account's uuid), and anyone holding a link to a
//! link-shared entry reads it at `GET /api/v1/public/{owner}/{path}`.
//!
//! Addresses keep working when things move:
//!
//! - an id the owner gave up answers `308` with the address under the
//!   owner's current id (Silicon Accounts reports every change by webhook);
//! - addresses from before personal drives, `/org/{org}/{path}` and
//!   `/api/v1/public/{org}/{path}`, resolve through the aliases the cutover
//!   recorded when it moved each organization's files into drives.
//!
//! Without `Authorization` an address serves only what anyone with the link
//! may read. An address never confirms that an entry the caller cannot read
//! exists: every such case answers `404`.

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse as _, Response},
};
use serde_json::json;

use super::{
    super::{
        auth::Action,
        drives,
        dto::{DispositionDto, PathContentQuery},
        extract,
        mapping::metadata_error,
        state::AppState,
    },
    content,
    sharing::{self, PublicQuery},
};
use crate::{
    application::content::ContentIntent,
    domain::{
        account::{PublicId, is_account_uuid},
        entry::EntryPath,
    },
    error::AppError,
    infrastructure::accounts::DriveOwner,
};

/// `GET /api/v1/paths/{owner}/{path}`: one entry, or its bytes with
/// `disposition`.
pub(crate) async fn resolve(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    location: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PathContentQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let (owner, path) = extract::path(location)?;
    let path = EntryPath::new(&path).map_err(|_| AppError::NotFound)?;
    let query = extract::query(query)?;
    resolve_in_owner(&state, &headers, &uri, &owner, path, query).await
}

/// `GET /api/v1/paths/{owner}`: the top of the owner's drive.
pub(crate) async fn resolve_root(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    owner: Result<Path<String>, PathRejection>,
    query: Result<Query<PathContentQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let owner = extract::path(owner)?;
    let query = extract::query(query)?;
    resolve_in_owner(
        &state,
        &headers,
        &uri,
        &owner,
        EntryPath::drive_root(),
        query,
    )
    .await
}

/// `GET /org/{org}/{path}`: an address from before personal drives.
pub(crate) async fn resolve_legacy(
    State(state): State<AppState>,
    headers: HeaderMap,
    location: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PathContentQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let (organization, path) = extract::path(location)?;
    let query = extract::query(query)?;
    let (drive, path) = legacy_target(&state, &organization, &path)
        .await?
        .ok_or(AppError::NotFound)?;
    let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
    serve(&state, &headers, &drive, path, query).await
}

/// `GET /api/v1/public/{owner}/{path}`: what anyone with the link may read.
/// `owner` is a `c:`/`si:` id, an account uuid, or a pre-migration
/// organization whose files moved into drives.
pub(crate) async fn public(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    location: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PublicQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let (owner, path) = extract::path(location)?;
    let query = extract::query(query)?;
    if PublicId::parse(&owner).is_ok() {
        let entry_path = EntryPath::new(&path).map_err(|_| AppError::NotFound)?;
        return match state.accounts.drive_owner(&owner).await? {
            DriveOwner::Drive(drive) => {
                sharing::read_public(&state, &headers, &drive, entry_path.as_str(), query).await
            }
            DriveOwner::Moved { uuid, id } => moved(
                &state,
                &state.mapper.public_api_url(&id, &entry_path, uri.query())?,
                &uuid,
                &id,
                &entry_path,
            ),
            DriveOwner::Unknown => Err(AppError::NotFound),
        };
    }
    if !is_account_uuid(&owner) {
        return Err(AppError::NotFound);
    }
    let repository = state.content_adapter.metadata_repository();
    if repository
        .drive_exists(&owner)
        .await
        .map_err(database_error)?
    {
        return sharing::read_public(&state, &headers, &owner, &path, query).await;
    }
    let (drive, path) = legacy_target(&state, &owner, &path)
        .await?
        .ok_or(AppError::NotFound)?;
    sharing::read_public(&state, &headers, &drive, &path, query).await
}

async fn resolve_in_owner(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    owner: &str,
    path: EntryPath,
    query: PathContentQuery,
) -> Result<Response, AppError> {
    match state.accounts.drive_owner(owner).await? {
        DriveOwner::Drive(drive) => serve(state, headers, &drive, path, query).await,
        DriveOwner::Moved { uuid, id } => moved(
            state,
            &state.mapper.api_path_url(&id, &path, uri.query())?,
            &uuid,
            &id,
            &path,
        ),
        DriveOwner::Unknown => Err(AppError::NotFound),
    }
}

/// Serves `path` in `drive`: anonymously what the link allows, otherwise
/// whatever the caller may read there.
async fn serve(
    state: &AppState,
    headers: &HeaderMap,
    drive: &str,
    path: EntryPath,
    query: PathContentQuery,
) -> Result<Response, AppError> {
    if !headers.contains_key(header::AUTHORIZATION) {
        if path.is_drive_root() {
            // The top of a drive never gets a link: it takes the caller's own
            // access token, like every other drive-wide read.
            return Err(super::super::auth::authentication_required());
        }
        let view = query.disposition.map(|value| match value {
            DispositionDto::Inline => "inline".to_owned(),
            DispositionDto::Attachment => "attachment".to_owned(),
        });
        return sharing::read_public(
            state,
            headers,
            drive,
            path.as_str(),
            PublicQuery { view, cursor: None },
        )
        .await;
    }
    let intent = query.disposition.map(content_intent);
    let action = intent.map_or(Action::ReadEntry, |intent| match intent {
        ContentIntent::Render => Action::ReadContent,
        ContentIntent::Download => Action::DownloadFile,
    });
    let resource = format!("{drive}/{}", path.as_str());
    let context = extract::authenticate(state, headers, action, &resource).await?;
    let context = drives::enter(state, context, drive).await?;
    if extract::tenant(&context) != drive {
        return Err(AppError::NotFound);
    }
    let entry = extract::scoped(&context, state.metadata.get_entry_by_path(&context, &path))
        .await
        .map_err(metadata_error)?;
    let Some(intent) = intent else {
        let mapper = state.mapper_for([drive]).await?;
        return Ok(Json(mapper.entry_item(drive, entry)?).into_response());
    };
    if entry.is_folder() && !matches!(intent, ContentIntent::Download) {
        return Err(AppError::NotFound);
    }
    content::serve(state, headers, &context, entry.id(), intent).await
}

/// Where a pre-migration address leads: the drive it was moved to, or a
/// drive key used in its place.
async fn legacy_target(
    state: &AppState,
    organization: &str,
    path: &str,
) -> Result<Option<(String, String)>, AppError> {
    let repository = state.content_adapter.metadata_repository();
    if is_account_uuid(organization)
        && repository
            .drive_exists(organization)
            .await
            .map_err(database_error)?
    {
        return Ok(Some((organization.to_owned(), path.to_owned())));
    }
    repository
        .legacy_location(organization, path)
        .await
        .map_err(database_error)
}

/// `308 Permanent Redirect` to the same address under the owner's current
/// id, with that address and the matching site address in the body.
fn moved(
    state: &AppState,
    api_url: &url::Url,
    uuid: &str,
    id: &str,
    path: &EntryPath,
) -> Result<Response, AppError> {
    let permanent_url = state
        .mapper
        .with_labels(std::collections::BTreeMap::from([(
            uuid.to_owned(),
            id.to_owned(),
        )]))
        .permanent_url(uuid, path)?;
    let location = HeaderValue::from_str(api_url.as_str()).map_err(|_| AppError::Internal {
        category: "moved_location",
    })?;
    let mut response = (
        StatusCode::PERMANENT_REDIRECT,
        Json(json!({
            "moved_to": api_url,
            "permanent_url": permanent_url,
            "owner": { "uuid": uuid, "id": id },
        })),
    )
        .into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

const fn content_intent(value: DispositionDto) -> ContentIntent {
    match value {
        DispositionDto::Inline => ContentIntent::Render,
        DispositionDto::Attachment => ContentIntent::Download,
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "passed straight to Result::map_err"
)]
fn database_error(error: sqlx::Error) -> AppError {
    tracing::error!(event = "location_lookup_failed", error = %error, "permanent address lookup failed");
    AppError::Internal {
        category: "location_lookup",
    }
}
