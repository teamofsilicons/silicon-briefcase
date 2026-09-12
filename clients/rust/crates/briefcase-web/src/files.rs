use crate::{App, Failure, Result, bad, session, staging};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use briefcase_client::{
    AccessRight, ActorRef, ByteRange, Destination, EffectiveAccess, EntryUpdate, IdempotencyKey,
    ListEntries, NewFolder, NewGrant, RootType, Upload,
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

fn json_value(value: impl serde::Serialize) -> Result<Json<Value>> {
    Ok(Json(
        serde_json::to_value(value).map_err(|_| bad("Unexpected response"))?,
    ))
}
#[derive(Deserialize, Default)]
pub(crate) struct Listing {
    path: Option<String>,
    filter: Option<String>,
    cursor: Option<String>,
}
pub(crate) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<Listing>,
) -> Result<Json<Value>> {
    let client = session::client(&app, &headers).await?;
    json_value(
        client
            .list_entries(&ListEntries {
                parent: q.path.filter(|p| !p.is_empty()).map(Destination::Path),
                filter: q.filter,
                cursor: q.cursor,
                limit: Some(100),
            })
            .await?,
    )
}
pub(crate) async fn stat(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    json_value(session::client(&app, &headers).await?.entry(id).await?)
}
#[derive(Deserialize)]
pub(crate) struct Resolve {
    path: String,
}
pub(crate) async fn resolve(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<Resolve>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .entry_at(&q.path)
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Folder {
    name: String,
    parent: String,
    root_type: Option<RootType>,
    tag: Option<String>,
    operation_id: Uuid,
}
pub(crate) async fn mkdir(
    State(app): State<App>,
    headers: HeaderMap,
    Json(q): Json<Folder>,
) -> Result<Json<Value>> {
    if q.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    let folder = if q.parent.is_empty() {
        let kind = q.root_type.ok_or_else(|| bad("Choose a folder type."))?;
        if kind == RootType::Tag {
            let tag = q
                .tag
                .filter(|tag| !tag.is_empty())
                .ok_or_else(|| bad("Choose a tag space."))?;
            NewFolder::in_tag(q.name, tag)
        } else {
            if q.tag.is_some() {
                return Err(bad("Only a tag folder can specify a tag."));
            }
            NewFolder::at_base(q.name, kind)
        }
    } else {
        if q.root_type.is_some() || q.tag.is_some() {
            return Err(bad("A nested folder inherits its parent's type."));
        }
        NewFolder::in_folder(q.name, Destination::Path(q.parent))
    }
    .with_idempotency_key(IdempotencyKey::new(q.operation_id.to_string())?);
    json_value(
        session::client(&app, &headers)
            .await?
            .create_folder(&folder)
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rename {
    name: Option<String>,
    parent_id: Option<Uuid>,
    operation_id: Uuid,
}
pub(crate) async fn rename(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(q): Json<Rename>,
) -> Result<Json<Value>> {
    if q.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    json_value(
        session::client(&app, &headers)
            .await?
            .update_entry_with_key(
                id,
                &EntryUpdate {
                    name: q.name,
                    parent_id: q.parent_id,
                },
                &IdempotencyKey::new(q.operation_id.to_string())?,
            )
            .await?,
    )
}
pub(crate) async fn trash(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    session::client(&app, &headers)
        .await?
        .delete_entry(id)
        .await?;
    Ok(Json(json!({"deleted":true})))
}
pub(crate) async fn bin(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<Listing>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .bin(q.cursor.as_deref(), Some(100))
            .await?,
    )
}
pub(crate) async fn restore_bin(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(q): Json<Intent>,
) -> Result<Json<Value>> {
    if q.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    json_value(
        session::client(&app, &headers)
            .await?
            .restore_from_bin_with_key(id, &IdempotencyKey::new(q.operation_id.to_string())?)
            .await?,
    )
}
pub(crate) async fn versions(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(page): Query<crate::access::Page>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .versions_page(id, page.cursor.as_deref())
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    operation_id: Uuid,
}
pub(crate) async fn restore_version(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, version)): Path<(Uuid, Uuid)>,
    Json(q): Json<Intent>,
) -> Result<Json<Value>> {
    if q.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    json_value(
        session::client(&app, &headers)
            .await?
            .restore_version_with_key(
                id,
                version,
                &IdempotencyKey::new(q.operation_id.to_string())?,
            )
            .await?,
    )
}
pub(crate) async fn activity(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    json_value(session::client(&app, &headers).await?.activity(id).await?)
}
pub(crate) async fn permissions(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .permissions(id)
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Grant {
    principal: ActorRef,
    access: Vec<AccessRight>,
    inherit: bool,
}
pub(crate) async fn grant(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(q): Json<Grant>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .grant(
                id,
                &NewGrant {
                    principal: q.principal,
                    access: q.access,
                    inherit: q.inherit,
                },
            )
            .await?,
    )
}
pub(crate) async fn revoke(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, grant)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>> {
    session::client(&app, &headers)
        .await?
        .revoke(id, grant)
        .await?;
    Ok(Json(json!({"revoked":true})))
}
#[derive(Deserialize)]
pub(crate) struct Search {
    q: String,
}
pub(crate) async fn search(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<Search>,
) -> Result<Json<Value>> {
    json_value(
        session::client(&app, &headers)
            .await?
            .search(&q.q, Some(20))
            .await?,
    )
}
pub(crate) async fn usage(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    json_value(session::client(&app, &headers).await?.usage().await?)
}

#[derive(Deserialize)]
pub(crate) struct Content {
    #[serde(default)]
    download: bool,
}
pub(crate) async fn content(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Query(q): Query<Content>,
) -> Result<Response> {
    let client = session::client(&app, &headers).await?;
    let entry = client.entry(id).await?;
    let range = requested_range(&headers, entry.size.unwrap_or(0))?;
    let content = if q.download && range.is_none() {
        client.download(id).await
    } else {
        client.read_content(id, range).await
    };
    let content = match content {
        Ok(content) => content,
        Err(briefcase_client::Error::Api(error)) if error.status == 416 => {
            let mut response =
                Failure(StatusCode::RANGE_NOT_SATISFIABLE, error.message).into_response();
            // Use the length reported for the actual read, not the earlier
            // metadata lookup: the file may have changed between requests.
            if let Some(length) = error.unsatisfied_range_length {
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{length}"))
                        .map_err(|_| bad("Invalid byte range"))?,
                );
            }
            response
                .headers_mut()
                .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return Ok(response);
        }
        Err(error) => return Err(error.into()),
    };
    let mime = content
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_owned();
    let length = content.content_length();
    let content_range = content.content_range().map(str::to_owned);
    let stream = stream::try_unfold(content, |mut content| async move {
        content
            .chunk()
            .await
            .map(|chunk| chunk.map(|chunk| (chunk, content)))
    });
    let mut response = Body::from_stream(stream).into_response();
    if content_range.is_some() {
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|_| bad("Invalid content type"))?,
    );
    if let Some(length) = length {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    }
    if let Some(range) = content_range {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&range).map_err(|_| bad("Invalid byte range"))?,
        );
    }
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let filename = if entry.entry_type == briefcase_client::EntryType::Folder {
        format!("{}.tar.zst", entry.name)
    } else {
        entry.name
    };
    let name: String = url::form_urlencoded::byte_serialize(filename.as_bytes()).collect();
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "{}; filename*=UTF-8''{}",
            if q.download { "attachment" } else { "inline" },
            name.replace('+', "%20")
        ))
        .map_err(|_| bad("Invalid filename"))?,
    );
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(if q.download {
            "sandbox allow-downloads; default-src 'none'; frame-ancestors 'none'; base-uri 'none'"
        } else {
            "sandbox; default-src 'none'; frame-ancestors 'self'; base-uri 'none'"
        }),
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadQuery {
    parent: String,
    name: String,
    content_type: String,
    operation_id: Uuid,
}
pub(crate) async fn upload(
    State(app): State<App>,
    Query(q): Query<UploadQuery>,
    request: Request,
) -> Result<Json<Value>> {
    let _permit = app.uploads.clone().try_acquire_owned().map_err(|_| {
        Failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Other uploads are in progress. Try again shortly.".into(),
        )
    })?;
    if q.operation_id.is_nil() || q.parent.is_empty() {
        return Err(bad("Choose a writable folder first."));
    }
    // Match the API's name normalization before accepting any file bytes.
    let name = q.name.trim();
    if name.is_empty()
        || name.len() > 255
        || name.contains(['\0', '/'])
        || matches!(name, "." | "..")
    {
        return Err(bad(
            "Invalid filename. Use a name of at most 255 UTF-8 bytes without a slash.",
        ));
    }
    if q.content_type.len() > 255 || q.content_type.parse::<mime::Mime>().is_err() {
        return Err(bad("Invalid content type."));
    }
    let (parts, body) = request.into_parts();
    if parts.headers.contains_key(header::CONTENT_ENCODING) {
        return Err(bad("Send unencoded file bytes."));
    }
    let declared = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| bad("Invalid upload length."))
        })
        .transpose()?;
    if declared.is_some_and(|size| size > staging::MAX_FILE_BYTES) {
        return Err(Failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "File exceeds 5 TiB.".into(),
        ));
    }
    let client = session::client(&app, &parts.headers).await?;
    let destination = client.entry_at(&q.parent).await?;
    if !destination.is_folder() || !destination.allows(EffectiveAccess::Write) {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "Choose a writable folder first.".into(),
        ));
    }
    // Declared bodies reserve their full size up front. Unframed bodies reserve
    // each chunk before writing; both consume the same aggregate byte budget.
    // Declaration order keeps this reservation alive until file cleanup.
    let mut reservation = app.staging.reserve(declared.unwrap_or(0))?;
    let temporary = tempfile::NamedTempFile::new_in(app.staging.directory()).map_err(|_| {
        Failure(
            StatusCode::INSUFFICIENT_STORAGE,
            "Temporary storage is unavailable.".into(),
        )
    })?;
    let mut file = tokio::fs::File::from_std(
        temporary
            .reopen()
            .map_err(|_| bad("Unable to stage upload"))?,
    );
    let mut body = body.into_data_stream();
    let staging = async {
        let mut size = 0_u64;
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(60), body.next())
                .await
                .map_err(|_| {
                    Failure(
                        StatusCode::REQUEST_TIMEOUT,
                        "Upload stopped sending data. Retry the same file.".into(),
                    )
                })?
        {
            let chunk = chunk.map_err(|_| bad("Upload interrupted. Retry the same file."))?;
            size += chunk.len() as u64;
            if size > staging::MAX_FILE_BYTES {
                return Err(Failure(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "File exceeds 5 TiB.".into(),
                ));
            }
            if declared.is_some_and(|declared| size > declared) {
                return Err(bad("Upload contains more bytes than declared."));
            }
            if declared.is_none() {
                reservation.grow(chunk.len() as u64)?;
            }
            file.write_all(&chunk).await.map_err(|_| {
                Failure(
                    StatusCode::INSUFFICIENT_STORAGE,
                    "Temporary storage is full.".into(),
                )
            })?;
            // Tokio's write may still be buffered in its blocking worker.
            file.flush()
                .await
                .map_err(|_| bad("Unable to finish upload staging"))?;
            reservation.written(chunk.len() as u64);
        }
        if declared.is_some_and(|declared| declared != size) {
            return Err(bad(
                "Upload ended before the declared file length. Retry the same file.",
            ));
        }
        file.flush()
            .await
            .map_err(|_| bad("Unable to finish upload staging"))?;
        Ok::<_, Failure>(())
    };
    tokio::time::timeout(app.staging.deadline, staging)
        .await
        .map_err(|_| {
            Failure(
                StatusCode::REQUEST_TIMEOUT,
                "Upload exceeded its deadline.".into(),
            )
        })??;
    // Refresh after the potentially long browser transfer and reject a session
    // removed by sign-out. The upstream still authorizes the final mutation.
    let client = session::client(&app, &parts.headers).await?;
    let current = client.entry_at(&q.parent).await?;
    if current.id != destination.id {
        return Err(Failure(
            StatusCode::CONFLICT,
            "The destination changed during upload. Choose the folder again.".into(),
        ));
    }
    if !current.is_folder() || !current.allows(EffectiveAccess::Write) {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "The destination is no longer writable.".into(),
        ));
    }
    let upload = Upload::file(Destination::Id(destination.id), temporary.path())?
        .named(name)
        .with_content_type(q.content_type)
        .with_idempotency_key(IdempotencyKey::new(q.operation_id.to_string())?);
    json_value(client.upload(&upload).await?)
}

pub(crate) fn requested_range(headers: &HeaderMap, size: u64) -> Result<Option<ByteRange>> {
    let range = if let Some(raw) = headers.get(header::RANGE) {
        let raw = raw
            .to_str()
            .map_err(|_| bad("Invalid byte range"))?
            .strip_prefix("bytes=")
            .ok_or_else(|| bad("Invalid byte range"))?;
        let (start, end) = raw
            .split_once('-')
            .ok_or_else(|| bad("Only one byte range is supported"))?;
        let start = if start.is_empty() {
            let tail: u64 = end.parse().map_err(|_| bad("Invalid byte range"))?;
            if tail == 0 {
                return Err(bad("Invalid byte range"));
            }
            size.saturating_sub(tail)
        } else {
            start.parse().map_err(|_| bad("Invalid byte range"))?
        };
        let end = if raw.starts_with('-') || end.is_empty() {
            None
        } else {
            Some(end.parse().map_err(|_| bad("Invalid byte range"))?)
        };
        Some(ByteRange { start, end })
    } else {
        None
    };
    Ok(range)
}
