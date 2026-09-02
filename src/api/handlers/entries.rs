//! Entry browsing, folder mutations, and recoverable-bin handlers.

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse as _, Response},
};
use uuid::Uuid;

use crate::{
    application::{
        content::ContentIntent,
        service::{
            CONTENTS_PAGE_SIZE, CreateFolderCommand, EntryListItem, InitialPermission,
            ListBinQuery, ListEntriesQuery as ServiceListEntriesQuery, PageRequest,
            RestoreBinEntryCommand, SearchQuery, UpdateEntryCommand,
        },
    },
    domain::{
        actor::{ActorId, ActorKind, ActorRef, TagName},
        entry::{EntryBoundary, EntryKind, EntryName, EntryPath},
        filter::FilterQuery,
        ids::EntryId,
        permission::{AccessRight, GrantedAccess},
    },
    error::AppError,
};

use super::{
    super::{
        auth::IamAction,
        cursor,
        dto::{
            ActivityPageDto, ActorRefDto, ActorTypeDto, DispositionDto, EntryDto, EntryPageDto,
            EntryPatchDto, FolderCreateDto, GrantAccessDto, ListEntriesQuery, PathContentQuery,
            PermissionGrantCreateDto, RootTypeDto, SearchPageDto, SearchQueryDto,
        },
        extract,
        mapping::metadata_error,
        state::AppState,
        validation,
    },
    content,
};

/// Lists folder contents, or the filtered organization view.
///
/// The default page is the hundred most recently changed entries. A `filter`
/// without a parent searches everything the caller may reach, which is how a
/// `location:` predicate selects a subtree.
pub(crate) async fn list_entries(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<ListEntriesQuery>, QueryRejection>,
) -> Result<Json<EntryPageDto>, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::list_entries(&query))?;
    if let Some(cursor) = query.cursor.as_deref() {
        cursor::validate_opaque(cursor).map_err(|_| AppError::bad_request("invalid_cursor"))?;
    }
    let resource = extract::organization_resource(&headers)?;
    let context =
        extract::authenticate(&state, &headers, IamAction::ListEntries, &resource).await?;
    let filter = query
        .filter
        .as_deref()
        .map(FilterQuery::parse)
        .transpose()
        .map_err(|_| AppError::validation("invalid_filter"))?;
    let parent_id = match (query.parent_id, query.path.as_deref()) {
        (Some(_), Some(_)) => return Err(AppError::bad_request("ambiguous_parent")),
        (Some(parent_id), None) => Some(extract::entry_id(parent_id)?),
        (None, Some(path)) => {
            let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
            let parent =
                extract::scoped(&context, state.metadata.get_entry_by_path(&context, &path))
                    .await
                    .map_err(metadata_error)?;
            if parent.entry.kind != EntryKind::Folder {
                return Err(AppError::NotFound);
            }
            Some(parent.entry.id)
        }
        (None, None) => None,
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

    // OpenAPI v1 has no structurally redacted traversal representation. Until
    // one is added, exposing a fabricated full Entry would leak protected
    // metadata, so traversal-only ancestors are intentionally omitted.
    let items = result
        .items
        .into_iter()
        .filter_map(|item| match item {
            EntryListItem::Full(entry) => Some(*entry),
            EntryListItem::Traversal(_) => None,
        })
        .map(|entry| state.mapper.entry(entry))
        .collect::<Result<_, _>>()?;
    Ok(Json(EntryPageDto {
        items,
        next_cursor: result.next_cursor,
    }))
}

pub(crate) async fn create_folder(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<FolderCreateDto>, JsonRejection>,
) -> Result<(StatusCode, Json<EntryDto>), AppError> {
    let body = extract::json(body)?;
    extract::validation(validation::create_folder(&body))?;
    let organization = extract::organization_resource(&headers)?;
    let context =
        extract::authenticate(&state, &headers, IamAction::CreateFolder, &organization).await?;
    // A parent may be addressed by identifier or by path; omitting both means
    // the organization base, where only a typed root may be created.
    let parent_id = match (body.parent_id, body.parent_path.as_deref()) {
        (Some(_), Some(_)) => return Err(AppError::bad_request("ambiguous_parent")),
        (Some(parent_id), None) => Some(extract::entry_id(parent_id)?),
        (None, Some(path)) => {
            let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
            let parent =
                extract::scoped(&context, state.metadata.get_entry_by_path(&context, &path))
                    .await
                    .map_err(metadata_error)?;
            if parent.entry.kind != EntryKind::Folder {
                return Err(AppError::NotFound);
            }
            Some(parent.entry.id)
        }
        (None, None) => None,
    };
    let resource = parent_id.map_or(organization, |id| id.to_string());
    let metadata = extract::mutation(&headers, "create_folder", &resource, &body, true)?;
    let command = folder_command(body, parent_id)?;
    let created = extract::scoped(
        &context,
        state.metadata.create_folder(&context, command, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok((StatusCode::CREATED, Json(state.mapper.entry(created)?)))
}

pub(crate) async fn get_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<EntryDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context = extract::authenticate(&state, &headers, IamAction::ReadEntry, &resource).await?;
    let entry = extract::scoped(&context, state.metadata.get_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry(entry)?))
}

/// Serves the contracted clean permanent URL `/org/{org_id}/{path}`.
///
/// The organization segment must match the authenticated tenant header, and an
/// entry the caller cannot read is reported exactly like a missing one. The
/// same URL returns the entry with its effective access by default, and the
/// sandboxed bytes when a disposition is requested.
pub(crate) async fn resolve_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PathContentQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let query = extract::query(query)?;
    let (organization, entry_path) = extract::entry_location(extract::path(path)?, &headers)?;
    let resource = format!("{organization}/{entry_path}");
    let intent = query.disposition.map(content_intent);
    let action = intent.map_or(IamAction::ReadEntry, |intent| match intent {
        ContentIntent::Render => IamAction::ReadContent,
        ContentIntent::Download => IamAction::DownloadFile,
    });
    let context = extract::authenticate(&state, &headers, action, &resource).await?;
    let entry = extract::scoped(
        &context,
        state.metadata.get_entry_by_path(&context, &entry_path),
    )
    .await
    .map_err(metadata_error)?;
    let Some(intent) = intent else {
        return Ok(Json(state.mapper.entry(entry)?).into_response());
    };
    if entry.entry.kind != EntryKind::File {
        return Err(AppError::NotFound);
    }
    content::serve(&state, &headers, &context, entry.entry.id, intent).await
}

const fn content_intent(value: DispositionDto) -> ContentIntent {
    match value {
        DispositionDto::Inline => ContentIntent::Render,
        DispositionDto::Attachment => ContentIntent::Download,
    }
}

/// Returns the retained "who did what, when" history of one entry.
pub(crate) async fn entry_activity(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ActivityPageDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::ListActivity, &resource).await?;
    let events = extract::scoped(&context, state.metadata.entry_activity(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::activity(
        events,
    )))
}

pub(crate) async fn update_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<EntryPatchDto>, JsonRejection>,
) -> Result<Json<EntryDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let body = extract::json(body)?;
    extract::validation(validation::patch_entry(&body))?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::UpdateEntry, &resource).await?;
    let metadata = extract::mutation(&headers, "update_entry", &resource, &body, true)?;
    let command = UpdateEntryCommand::new(
        entry_id,
        body.name
            .as_deref()
            .map(EntryName::new)
            .transpose()
            .map_err(|_| AppError::validation("invalid_name"))?,
        body.parent_id.map(extract::entry_id).transpose()?,
    )
    .map_err(|_| AppError::validation("invalid_entry_patch"))?;
    let updated = extract::scoped(
        &context,
        state.metadata.update_entry(&context, &command, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry(updated)?))
}

pub(crate) async fn delete_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::DeleteEntry, &resource).await?;
    let metadata = extract::mutation(&headers, "delete_entry", &resource, &(), false)?;
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

pub(crate) async fn list_bin(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<EntryPageDto>, AppError> {
    let resource = extract::organization_resource(&headers)?;
    let context = extract::authenticate(&state, &headers, IamAction::ListBin, &resource).await?;
    let result = extract::scoped(
        &context,
        state.metadata.list_bin(
            &context,
            &ListBinQuery {
                page: PageRequest::default(),
            },
        ),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry_page(result)?))
}

pub(crate) async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<SearchQueryDto>, QueryRejection>,
) -> Result<Json<SearchPageDto>, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::search(&query))?;
    let resource = extract::organization_resource(&headers)?;
    let context = extract::authenticate(&state, &headers, IamAction::Search, &resource).await?;
    let query = SearchQuery::new(query.q, query.limit.unwrap_or(20))
        .map_err(|_| AppError::validation("invalid_search"))?;
    let results = extract::scoped(&context, state.metadata.search(&context, &query))
        .await
        .map_err(metadata_error)?;
    Ok(Json(state.mapper.search(results)?))
}

pub(crate) async fn restore_bin_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<EntryDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::RestoreBinEntry, &resource).await?;
    let metadata = extract::mutation(&headers, "restore_bin_entry", &resource, &(), false)?;
    let restored = extract::scoped(
        &context,
        state
            .metadata
            .restore_bin_entry(&context, RestoreBinEntryCommand { entry_id }, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry(restored)?))
}

fn folder_command(
    body: FolderCreateDto,
    parent_id: Option<EntryId>,
) -> Result<CreateFolderCommand, AppError> {
    let name = EntryName::new(&body.name).map_err(|_| AppError::validation("invalid_name"))?;
    let root_boundary = match body.root_type {
        Some(RootTypeDto::Public) => Some(EntryBoundary::Public),
        Some(RootTypeDto::Private) => Some(EntryBoundary::Private),
        Some(RootTypeDto::Tag) => Some(EntryBoundary::Tag {
            tag: TagName::new(body.tag.unwrap_or_default())
                .map_err(|_| AppError::validation("invalid_tag"))?,
        }),
        None => None,
    };
    let invitees = body
        .invitees
        .into_iter()
        .map(initial_permission)
        .collect::<Result<_, _>>()?;
    CreateFolderCommand::new(name, parent_id, root_boundary, invitees)
        .map_err(|_| AppError::validation("invalid_folder"))
}

fn initial_permission(value: PermissionGrantCreateDto) -> Result<InitialPermission, AppError> {
    Ok(InitialPermission {
        principal: actor(value.principal)?,
        access: granted_access(&value.access)?,
        inherits_to_descendants: value.inherit,
    })
}

pub(crate) fn actor(value: ActorRefDto) -> Result<ActorRef, AppError> {
    let kind = match value.actor_type {
        ActorTypeDto::Carbon => ActorKind::Carbon,
        ActorTypeDto::Silicon => ActorKind::Silicon,
        ActorTypeDto::Application => {
            return Err(AppError::validation("invalid_principal_type"));
        }
    };
    let id = ActorId::new(value.id).map_err(|_| AppError::validation("invalid_principal_id"))?;
    Ok(ActorRef::new(kind, id))
}

/// Converts the requested right names into a validated access set.
///
/// An empty set would convey nothing, so it is rejected rather than silently
/// treated as read-only.
pub(crate) fn granted_access(values: &[GrantAccessDto]) -> Result<GrantedAccess, AppError> {
    if values.is_empty() {
        return Err(AppError::validation("invalid_access"));
    }
    Ok(GrantedAccess::new(values.iter().map(|value| match value {
        GrantAccessDto::Read => AccessRight::Read,
        GrantAccessDto::Write => AccessRight::Write,
        GrantAccessDto::Update => AccessRight::Update,
        GrantAccessDto::Delete => AccessRight::Delete,
    })))
}
