//! Permission grant handlers.

use axum::{
    Json,
    extract::{
        Path, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
};
use uuid::Uuid;

use crate::{
    application::{
        context::ExecutionContext,
        service::{
            GrantPermissionCommand, InspectPermissionsQuery, ListPermissionsQuery, PageRequest,
            RevokePermissionCommand,
        },
    },
    domain::{entry::EntryPath, ids::EntryId},
    error::AppError,
};

use super::{
    super::{
        auth::Action,
        drives,
        dto::{
            PermissionGrantCreateDto, PermissionGrantDto, PermissionGrantPageDto,
            PermissionInspectionDto, PermissionInspectionResultDto,
        },
        extract,
        mapping::metadata_error,
        state::AppState,
        validation,
    },
    entries::{actor, granted_access},
};

pub(crate) async fn list_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<PermissionGrantPageDto>, AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, Action::ListPermissions, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let page = extract::scoped(
        &context,
        state.metadata.list_permissions(
            &context,
            &ListPermissionsQuery {
                entry_id,
                page: PageRequest::default(),
            },
        ),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::permissions(
        &page.items,
    )))
}

/// Reports the caller's effective access on a batch of files and folders.
pub(crate) async fn inspect_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<PermissionInspectionDto>, JsonRejection>,
) -> Result<Json<PermissionInspectionResultDto>, AppError> {
    let body = extract::json(body)?;
    extract::validation(validation::inspect_permissions(&body))?;
    let context =
        extract::authenticate(&state, &headers, Action::InspectPermissions, "permissions").await?;
    let entry_ids = body
        .entry_ids
        .iter()
        .copied()
        .map(extract::entry_id)
        .collect::<Result<Vec<_>, _>>()?;
    let paths = body
        .paths
        .iter()
        .map(|path| EntryPath::new(path).map_err(|_| AppError::validation("invalid_path")))
        .collect::<Result<Vec<_>, _>>()?;
    InspectPermissionsQuery::new(entry_ids.clone(), paths.clone())
        .map_err(|_| AppError::validation("invalid_targets"))?;
    // Targets can sit in several drives: each drive answers for its own
    // entries, inside that drive, and paths are read in the named drive.
    let path_context = drives::named(&state, context.clone(), body.drive.as_deref()).await?;
    let path_drive = extract::tenant(&path_context);
    let mut groups: Vec<(ExecutionContext, Vec<EntryId>, Vec<EntryPath>)> =
        vec![(path_context, Vec::new(), paths.clone())];
    for entry_id in &entry_ids {
        let entered = drives::for_entry(&state, context.clone(), *entry_id).await?;
        let drive = extract::tenant(&entered);
        if drive == path_drive {
            groups[0].1.push(*entry_id);
        } else if let Some(group) = groups
            .iter_mut()
            .find(|(group_context, _, _)| extract::tenant(group_context) == drive)
        {
            group.1.push(*entry_id);
        } else {
            groups.push((entered, vec![*entry_id], Vec::new()));
        }
    }
    let mut visible = Vec::new();
    for (group_context, group_ids, group_paths) in groups {
        if group_ids.is_empty() && group_paths.is_empty() {
            continue;
        }
        let query = InspectPermissionsQuery::new(group_ids, group_paths)
            .map_err(|_| AppError::validation("invalid_targets"))?;
        visible.extend(
            extract::scoped(
                &group_context,
                state.metadata.inspect_permissions(&group_context, &query),
            )
            .await
            .map_err(metadata_error)?,
        );
    }
    Ok(Json(super::super::mapping::ResponseMapper::inspection(
        &entry_ids, &paths, visible,
    )))
}

pub(crate) async fn grant_permission(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<PermissionGrantCreateDto>, JsonRejection>,
) -> Result<(StatusCode, Json<PermissionGrantDto>), AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let body = extract::json(body)?;
    extract::validation(validation::grant_permission(&body))?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, Action::GrantPermission, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let metadata = extract::mutation(&headers, "grant_permission", &resource, &body, false)?;
    let named = actor(body.principal)?;
    let principal =
        extract::resolve_recipient(&state, &context, named.kind(), named.id().as_str()).await?;
    let command = GrantPermissionCommand {
        entry_id,
        principal,
        access: granted_access(&body.access)?,
        inherits_to_descendants: body.inherit,
        lifetime: body.expires_in_minutes,
    };
    let context = extract::with_recipients(context, std::slice::from_ref(&command.principal));
    let grant = extract::scoped(
        &context,
        state
            .metadata
            .grant_permission(&context, &command, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok((
        StatusCode::CREATED,
        Json(super::super::mapping::ResponseMapper::permission(&grant)),
    ))
}

pub(crate) async fn revoke_permission(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(Uuid, Uuid)>, PathRejection>,
) -> Result<StatusCode, AppError> {
    let (entry_uuid, grant_uuid) = extract::path(path)?;
    let entry_id = extract::entry_id(entry_uuid)?;
    let grant_id = extract::grant_id(grant_uuid)?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, Action::RevokePermission, &resource).await?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let metadata = extract::mutation(&headers, "revoke_permission", &resource, &grant_uuid, false)?;
    extract::scoped(
        &context,
        state.metadata.revoke_permission(
            &context,
            RevokePermissionCommand { entry_id, grant_id },
            &metadata,
        ),
    )
    .await
    .map_err(metadata_error)?;
    Ok(StatusCode::NO_CONTENT)
}
