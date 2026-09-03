//! Permission grants and access-request workflow handlers.

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
    application::service::{
        DecideAccessRequestCommand, GrantPermissionCommand, InspectPermissionsQuery,
        ListPermissionsQuery, PageRequest, RequestAccessCommand, RevokePermissionCommand,
    },
    domain::{access::AccessDecision, entry::EntryPath},
    error::AppError,
};

use super::{
    super::{
        auth::IamAction,
        dto::{
            AccessDecisionDto, AccessRequestCreateDto, AccessRequestDecisionDto, AccessRequestDto,
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
        extract::authenticate(&state, &headers, IamAction::ListPermissions, &resource).await?;
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
    let resource = extract::organization_resource(&headers)?;
    let context =
        extract::authenticate(&state, &headers, IamAction::InspectPermissions, &resource).await?;
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
    let query = InspectPermissionsQuery::new(entry_ids.clone(), paths.clone())
        .map_err(|_| AppError::validation("invalid_targets"))?;
    let visible = extract::scoped(
        &context,
        state.metadata.inspect_permissions(&context, &query),
    )
    .await
    .map_err(metadata_error)?;
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
        extract::authenticate(&state, &headers, IamAction::GrantPermission, &resource).await?;
    let metadata = extract::mutation(&headers, "grant_permission", &resource, &body, false)?;
    let command = GrantPermissionCommand {
        entry_id,
        principal: actor(body.principal)?,
        access: granted_access(&body.access)?,
        inherits_to_descendants: body.inherit,
    };
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
        extract::authenticate(&state, &headers, IamAction::RevokePermission, &resource).await?;
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

pub(crate) async fn request_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<AccessRequestCreateDto>, JsonRejection>,
) -> Result<(StatusCode, Json<AccessRequestDto>), AppError> {
    let entry_id = extract::entry_id(extract::path(path)?)?;
    let body = extract::json(body)?;
    extract::validation(validation::request_access(&body))?;
    let resource = entry_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::CreateAccessRequest, &resource).await?;
    let metadata = extract::mutation(&headers, "request_access", &resource, &body, false)?;
    let access = granted_access(&body.access)?;
    let command = RequestAccessCommand::new(entry_id, access, body.reason)
        .map_err(|_| AppError::validation("invalid_access_request"))?;
    let request = extract::scoped(
        &context,
        state.metadata.request_access(&context, &command, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    Ok((
        StatusCode::CREATED,
        Json(super::super::mapping::ResponseMapper::access_request(
            &request,
        )),
    ))
}

pub(crate) async fn decide_access_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<AccessRequestDecisionDto>, JsonRejection>,
) -> Result<Json<AccessRequestDto>, AppError> {
    let request_id = extract::access_request_id(extract::path(path)?)?;
    let body = extract::json(body)?;
    extract::validation(validation::decide_access(&body))?;
    let resource = request_id.to_string();
    let context =
        extract::authenticate(&state, &headers, IamAction::DecideAccessRequest, &resource).await?;
    let metadata = extract::mutation(&headers, "decide_access_request", &resource, &body, false)?;
    let decision = match body.decision {
        AccessDecisionDto::Approve => AccessDecision::Approve {
            access: granted_access(
                body.access
                    .as_deref()
                    .ok_or_else(|| AppError::validation("missing_approved_access"))?,
            )?,
        },
        AccessDecisionDto::Deny => AccessDecision::Deny,
    };
    let request = extract::scoped(
        &context,
        state.metadata.decide_access_request(
            &context,
            DecideAccessRequestCommand {
                request_id,
                decision,
            },
            &metadata,
        ),
    )
    .await
    .map_err(metadata_error)?;
    Ok(Json(super::super::mapping::ResponseMapper::access_request(
        &request,
    )))
}
