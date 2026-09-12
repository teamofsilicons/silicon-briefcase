//! Read-only public-link access and authenticated sharing/log controls.

use super::super::{auth::IamAction, delivery, dto::PageQuery, extract, state::AppState};
use crate::{
    application::{
        content::{ContentDelivery, ContentIntent, map_object_error, resolve_range},
        context::TestingEnvironmentContext,
        ports::OpenObjectRequest,
    },
    domain::{actor::is_canonical_iam_organization_id, entry::EntryPath},
    error::AppError,
    infrastructure::postgres::{
        TenantContext,
        sharing::{LinkAccess, LogPage},
    },
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkUpdate {
    pub enabled: bool,
}

pub(crate) async fn link_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<LinkAccess>, AppError> {
    let context =
        extract::authenticate(&state, &headers, IamAction::ReadEntry, &id.to_string()).await?;
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .link_access(&context, extract::entry_id(id)?)
            .await?,
    ))
}

pub(crate) async fn set_link_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<LinkUpdate>,
) -> Result<Json<LinkAccess>, AppError> {
    let context = extract::authenticate(
        &state,
        &headers,
        IamAction::GrantPermission,
        &id.to_string(),
    )
    .await?;
    let metadata = extract::mutation(&headers, "set_link_access", &id.to_string(), &body, true)?;
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .set_link_access(&context, extract::entry_id(id)?, body.enabled, &metadata)
            .await?,
    ))
}

pub(crate) async fn logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Json<LogPage>, AppError> {
    let context =
        extract::authenticate(&state, &headers, IamAction::ListActivity, &id.to_string()).await?;
    let cursor = query
        .cursor
        .map(|v| {
            v.parse()
                .map_err(|_| AppError::bad_request("invalid_cursor"))
        })
        .transpose()?;
    let limit = query.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(AppError::validation("invalid_limit"));
    }
    Ok(Json(
        state
            .content_adapter
            .metadata_repository()
            .logs(&context, extract::entry_id(id)?, cursor, limit)
            .await?,
    ))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicQuery {
    /// metadata, contents, inline, or attachment.
    pub view: Option<String>,
    pub cursor: Option<Uuid>,
}

pub(crate) async fn public_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((org, path)): Path<(String, String)>,
    Query(query): Query<PublicQuery>,
) -> Result<Response, AppError> {
    read_public(&state, &headers, &org, &path, query).await
}

pub(crate) async fn read_public(
    state: &AppState,
    headers: &HeaderMap,
    org: &str,
    path: &str,
    query: PublicQuery,
) -> Result<Response, AppError> {
    if !is_canonical_iam_organization_id(org) {
        return Err(AppError::NotFound);
    }
    let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
    let access = extract::optional_testing_access(state, headers).await?;
    if access
        .as_ref()
        .is_some_and(|access| access.owner_org_id != org)
    {
        return Err(AppError::NotFound);
    }
    let fence = extract::testing_use_fence(state, access.as_ref()).await?;
    extract::touch_testing_access(state, access.as_ref()).await?;
    let tenant = if let Some(access) = access {
        TenantContext::for_testing_environment_service(
            org,
            TestingEnvironmentContext::new(access.environment_id, access.control_version),
            "public-link",
        )
    } else {
        TenantContext::for_control_service(org, "public-link")
    };
    let repo = state.content_adapter.metadata_repository();
    let mut response = match query.view.as_deref().unwrap_or("metadata") {
        "metadata" => Ok(Json(repo.public_entry(&tenant, path.as_str()).await?).into_response()),
        "contents" => Ok(Json(
            repo.public_children(&tenant, path.as_str(), query.cursor)
                .await?,
        )
        .into_response()),
        "inline" | "attachment" => {
            let intent = if query.view.as_deref() == Some("attachment") {
                ContentIntent::Download
            } else {
                ContentIntent::Render
            };
            let entry = repo.public_entry(&tenant, path.as_str()).await?;
            if entry.entry_type == "folder" && matches!(intent, ContentIntent::Download) {
                let response =
                    super::archive::serve_public(state, tenant, path.into_inner(), headers).await?;
                return Ok(retain_fence(response, fence));
            }
            let content = open_public(
                state,
                &tenant,
                path.as_str(),
                delivery::requested_range(headers)?,
            )
            .await?;
            delivery::response(content, intent)
        }
        _ => Err(AppError::bad_request("invalid_view")),
    }?;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(retain_fence(response, fence))
}

fn retain_fence(
    response: Response,
    fence: Option<crate::infrastructure::testing::TestingEnvironmentUseFence>,
) -> Response {
    use futures::StreamExt as _;
    let (parts, body) = response.into_parts();
    let stream = futures::stream::unfold(
        (body.into_data_stream(), fence),
        |(mut body, fence)| async move { body.next().await.map(|chunk| (chunk, (body, fence))) },
    );
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

pub(super) async fn open_public(
    state: &AppState,
    tenant: &TenantContext,
    path: &str,
    range: Option<crate::application::ports::RangeRequest>,
) -> Result<ContentDelivery, AppError> {
    let target = state.content_adapter.public_download(tenant, path).await?;
    let range = range.map(|r| resolve_range(r, target.size)).transpose()?;
    let object = state
        .objects
        .open_object(OpenObjectRequest {
            target: &target.target,
            key: &target.key,
            provider_version_id: target.provider_version_id.as_deref(),
            range,
        })
        .await
        .map_err(|e| map_object_error(&e))?;
    Ok(ContentDelivery {
        filename: target.filename,
        content_type: target.content_type,
        total_size: object.total_size,
        range: object.range,
        etag: object.etag,
        body: object.body,
    })
}
