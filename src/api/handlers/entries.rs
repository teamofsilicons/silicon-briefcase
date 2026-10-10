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
            CONTENTS_PAGE_SIZE, CreateFolderCommand, InitialPermission, ListBinQuery,
            ListEntriesQuery as ServiceListEntriesQuery, PageRequest, RestoreBinEntryCommand,
            SearchQuery, UpdateEntryCommand,
        },
    },
    domain::{
        account::is_account_uuid,
        actor::{ActorId, ActorKind, ActorRef},
        entry::{EntryName, EntryPath},
        filter::FilterQuery,
        ids::EntryId,
        permission::{AccessRight, GrantedAccess},
    },
    error::AppError,
};

use super::{
    super::{
        auth::Action,
        cursor, drives,
        dto::{
            ActivityPageDto, ActorRefDto, ActorTypeDto, DispositionDto, DrivePageQuery, EntryDto,
            EntryPageDto, EntryPatchDto, FolderCreateDto, GrantAccessDto, ListEntriesQuery,
            PathContentQuery, PermissionGrantCreateDto, RootTypeDto, SearchPageDto, SearchQueryDto,
        },
        extract,
        mapping::metadata_error,
        state::AppState,
        validation,
    },
    content,
};

/// Lists folder contents, the top of a drive, or a filtered view of a drive.
///
/// The default page is the hundred most recently changed entries. A `filter`
/// without a parent searches everything in the drive the caller may reach,
/// which is how a `location:` predicate selects a subtree. `drive` names
/// another account's drive (default: the caller's own).
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
    let context = extract::authenticate(&state, &headers, Action::ListEntries, "entries").await?;
    let context = match query.parent_id {
        Some(parent_id) => {
            drives::for_entry(&state, context, extract::entry_id(parent_id)?).await?
        }
        None => drives::named(&state, context, query.drive.as_deref()).await?,
    };
    let resource = extract::tenant(&context);
    let filter = filter_query(&state, query.filter.as_deref()).await?;
    let parent_id = match (query.parent_id, query.path.as_deref()) {
        (Some(_), Some(_)) => return Err(AppError::bad_request("ambiguous_parent")),
        (Some(parent_id), None) => Some(extract::entry_id(parent_id)?),
        (None, Some(path)) => {
            let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
            let parent =
                extract::scoped(&context, state.metadata.get_entry_by_path(&context, &path))
                    .await
                    .map_err(metadata_error)?;
            if !parent.is_folder() {
                return Err(AppError::NotFound);
            }
            Some(parent.id())
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

    // A folder shared only through its contents is listed as a traversal
    // entry: the caller can open it and see what was shared, and nothing else
    // about it is disclosed.
    let items = result
        .items
        .into_iter()
        .map(|item| state.mapper.entry_item(&resource, item))
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
    refuse_retired_spaces(&body)?;
    let context = extract::authenticate(&state, &headers, Action::CreateFolder, "entries").await?;
    let context = match body.parent_id {
        Some(parent_id) => {
            drives::for_entry(&state, context, extract::entry_id(parent_id)?).await?
        }
        None => drives::named(&state, context, body.drive.as_deref()).await?,
    };
    let organization = extract::tenant(&context);
    // A parent may be addressed by identifier or by path; omitting both
    // creates the folder at the top of the drive.
    let parent_id = match (body.parent_id, body.parent_path.as_deref()) {
        (Some(_), Some(_)) => return Err(AppError::bad_request("ambiguous_parent")),
        (Some(parent_id), None) => Some(extract::entry_id(parent_id)?),
        (None, Some(path)) => {
            let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
            let parent =
                extract::scoped(&context, state.metadata.get_entry_by_path(&context, &path))
                    .await
                    .map_err(metadata_error)?;
            if !parent.is_folder() {
                return Err(AppError::NotFound);
            }
            Some(parent.id())
        }
        (None, None) => None,
    };
    let resource = parent_id.map_or(organization, |id| id.to_string());
    let metadata = extract::mutation(&headers, "create_folder", &resource, &body, true)?;
    let mut command = folder_command(body, parent_id)?;
    let mut recipients = Vec::with_capacity(command.invitees.len());
    for invitee in &mut command.invitees {
        let (kind, id) = (
            invitee.principal.kind(),
            invitee.principal.id().as_str().to_owned(),
        );
        invitee.principal = extract::resolve_recipient(&state, &context, kind, &id).await?;
        recipients.push(invitee.principal.clone());
    }
    let context = extract::with_recipients(context, &recipients);
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
    let context = extract::authenticate(&state, &headers, Action::ReadEntry, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let organization = extract::tenant(&context);
    let entry = extract::scoped(&context, state.metadata.visible_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry_item(&organization, entry)?))
}

/// Serves the permanent URL `/org/{drive}/{path}`, where `drive` is the
/// drive owner's account uuid.
///
/// An entry the caller cannot read is reported exactly like a missing one.
/// The same URL returns the entry with its effective access by default, and
/// the sandboxed bytes when a disposition is requested.
pub(crate) async fn resolve_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, String)>, PathRejection>,
    query: Result<Query<PathContentQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let query = extract::query(query)?;
    let location = extract::path(path)?;
    if !headers.contains_key(axum::http::header::AUTHORIZATION) {
        return super::sharing::read_public(
            &state,
            &headers,
            &location.0,
            &location.1,
            super::sharing::PublicQuery {
                view: query.disposition.map(|value| match value {
                    DispositionDto::Inline => "inline".to_owned(),
                    DispositionDto::Attachment => "attachment".to_owned(),
                }),
                cursor: None,
            },
        )
        .await;
    }
    let resource = format!("{}/{}", location.0, location.1);
    let intent = query.disposition.map(content_intent);
    let action = intent.map_or(Action::ReadEntry, |intent| match intent {
        ContentIntent::Render => Action::ReadContent,
        ContentIntent::Download => Action::DownloadFile,
    });
    let context = extract::authenticate(&state, &headers, action, &resource).await?;
    if !is_account_uuid(&location.0) {
        return Err(AppError::NotFound);
    }
    let context = drives::enter(&state, context, &location.0).await?;
    let (organization, entry_path) = extract::entry_location(location, &context)?;
    let entry = extract::scoped(
        &context,
        state.metadata.get_entry_by_path(&context, &entry_path),
    )
    .await
    .map_err(metadata_error)?;
    let Some(intent) = intent else {
        return Ok(Json(state.mapper.entry_item(&organization, entry)?).into_response());
    };
    if entry.is_folder() && !matches!(intent, ContentIntent::Download) {
        return Err(AppError::NotFound);
    }
    content::serve(&state, &headers, &context, entry.id(), intent).await
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
    let context = extract::authenticate(&state, &headers, Action::ListActivity, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
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
    let context = extract::authenticate(&state, &headers, Action::UpdateEntry, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
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
    let context = extract::authenticate(&state, &headers, Action::DeleteEntry, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
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

/// Stops a file's self-destruct timer so the file is kept.
pub(crate) async fn make_permanent(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context = extract::authenticate(&state, &headers, Action::KeepEntry, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let metadata = extract::mutation(&headers, "make_permanent", &resource, &(), false)?;
    state
        .content_adapter
        .metadata_repository()
        .make_permanent(&context, entry_id, &metadata)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Lists a drive's recoverable entries, newest deletion first, one page at a
/// time: everything for the drive's owner (and the custodian of a Silicon
/// that owns it), and what they created for anyone else.
pub(crate) async fn list_bin(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<DrivePageQuery>, QueryRejection>,
) -> Result<Json<EntryPageDto>, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::drive_page(&query))?;
    if let Some(cursor) = query.cursor.as_deref() {
        cursor::validate_opaque(cursor).map_err(|_| AppError::bad_request("invalid_cursor"))?;
    }
    let context = extract::authenticate(&state, &headers, Action::ListBin, "bin").await?;
    let context = drives::named(&state, context, query.drive.as_deref()).await?;
    let page = PageRequest::new(query.cursor, query.limit.unwrap_or(CONTENTS_PAGE_SIZE))
        .map_err(|_| AppError::validation("invalid_pagination"))?;
    let result = extract::scoped(
        &context,
        state.metadata.list_bin(&context, &ListBinQuery { page }),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(state.mapper.entry_page(result)?))
}

/// Searches everything the caller can reach: its own drive, the drives of
/// the Silicons it looks after, and the drives where others shared something
/// with it (the most recently shared first, up to a bound).
pub(crate) async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Result<Query<SearchQueryDto>, QueryRejection>,
) -> Result<Json<SearchPageDto>, AppError> {
    let query = extract::query(query)?;
    extract::validation(validation::search(&query))?;
    let context = extract::authenticate(&state, &headers, Action::Search, "search").await?;
    let query = SearchQuery::new(query.q, query.limit.unwrap_or(20))
        .map_err(|_| AppError::validation("invalid_search"))?;
    let contexts = search_scope(&state, context.clone()).await?;
    let results = extract::scoped(&context, state.metadata.search_drives(&contexts, &query))
        .await
        .map_err(metadata_error)?;
    Ok(Json(state.mapper.search(results)?))
}

/// How many drives besides the caller's own and its Silicons' one search
/// covers: the drives with the most recent shares to the caller.
const MAX_SHARED_DRIVES_SEARCHED: i32 = 50;

async fn search_scope(
    state: &AppState,
    context: crate::application::context::ExecutionContext,
) -> Result<Vec<crate::application::context::ExecutionContext>, AppError> {
    let own = drives::own(state, context.clone()).await?;
    let mut keys = vec![own.authorization().organization_id().as_str().to_owned()];
    let mut contexts = vec![own];
    let Some(account) = context.authorization().account().cloned() else {
        return Ok(contexts);
    };
    let mut candidates = Vec::new();
    if account.kind == ActorKind::Carbon {
        candidates.extend(
            state
                .accounts
                .custodial_silicons(&account.uuid)
                .await?
                .into_iter()
                .map(|record| record.uuid),
        );
    }
    candidates.extend(
        state
            .content_adapter
            .metadata_repository()
            .shared_drives(context.authorization().actor(), MAX_SHARED_DRIVES_SEARCHED)
            .await
            .map_err(|_| AppError::Internal {
                category: "drive_lookup",
            })?,
    );
    for drive in candidates {
        if keys.contains(&drive) {
            continue;
        }
        match drives::enter(state, context.clone(), &drive).await {
            Ok(entered) => {
                keys.push(drive);
                contexts.push(entered);
            }
            Err(AppError::NotFound) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(contexts)
}

pub(crate) async fn restore_bin_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<EntryDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, Action::RestoreBinEntry, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
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

/// Folders used to be created in a Public space or a tag's space. Personal
/// drives have neither; say exactly what to do instead.
fn refuse_retired_spaces(body: &FolderCreateDto) -> Result<(), AppError> {
    if body.tag.is_some() || body.root_type == Some(RootTypeDto::Tag) {
        return Err(AppError::unprocessable(
            "tags_retired",
            "Tags no longer exist in Briefcase. Create the folder, then share it with each Carbon or Silicon by id.",
        ));
    }
    if body.root_type == Some(RootTypeDto::Public) {
        return Err(AppError::unprocessable(
            "public_space_retired",
            "There is no Public space: every drive is private to its owner. Create the folder, then share it with each Carbon or Silicon by id, or turn on link access so anyone with the link can view it.",
        ));
    }
    Ok(())
}

/// Parses a listing filter and names the accounts it mentions by uuid:
/// `from:@c:ada` means the account `c:ada` is now, resolved at Silicon
/// Accounts, because public ids can change hands.
pub(crate) async fn filter_query(
    state: &AppState,
    filter: Option<&str>,
) -> Result<Option<FilterQuery>, AppError> {
    let Some(filter) = filter else {
        return Ok(None);
    };
    let mut filter =
        FilterQuery::parse(filter).map_err(|_| AppError::validation("invalid_filter"))?;
    for selector in filter.actor_selectors_mut() {
        let record = state.accounts.resolve(&selector.id).await?;
        selector.kind = Some(record.kind);
        selector.id = record.uuid;
    }
    Ok(Some(filter))
}

fn folder_command(
    body: FolderCreateDto,
    parent_id: Option<EntryId>,
) -> Result<CreateFolderCommand, AppError> {
    let name = EntryName::new(&body.name).map_err(|_| AppError::validation("invalid_name"))?;
    let invitees = body
        .invitees
        .into_iter()
        .map(initial_permission)
        .collect::<Result<_, _>>()?;
    CreateFolderCommand::new(name, parent_id, invitees)
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
