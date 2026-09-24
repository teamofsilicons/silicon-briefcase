use crate::{App, Failure, Result, bad, lifetime, session, staging};
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
/// Keeps a self-destructing file: its timer stops and it is never deleted.
pub(crate) async fn keep(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    match session::client(&app, &headers)
        .await?
        .make_permanent(id)
        .await
    {
        Ok(()) => Ok(Json(json!({"kept":true}))),
        Err(briefcase_client::Error::Api(error)) if error.status == 403 => Err(Failure(
            StatusCode::FORBIDDEN,
            "Only the person who uploaded this file, an org admin or an org owner can keep it."
                .into(),
        )),
        Err(error) => Err(error.into()),
    }
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
    expires_in_minutes: Option<u32>,
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
                    expires_in_minutes: q.expires_in_minutes.map(lifetime).transpose()?,
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
    self_destruct_minutes: Option<u32>,
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
    let self_destruct = q.self_destruct_minutes.map(lifetime).transpose()?;
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
    // Self destruct is refused for a new version; say so before the transfer
    // rather than after it. The API still makes the authoritative decision.
    if self_destruct.is_some() {
        match client
            .entry_at(&format!("{}/{name}", destination.path))
            .await
        {
            Ok(existing) if !existing.is_folder() => {
                return Err(Failure(StatusCode::CONFLICT, crate::NEW_FILE_ONLY.into()));
            }
            Ok(_) => {}
            Err(briefcase_client::Error::Api(error)) if error.status == 404 => {}
            Err(error) => return Err(error.into()),
        }
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
    let mut upload = Upload::file(Destination::Id(destination.id), temporary.path())?
        .named(name)
        .with_content_type(q.content_type)
        .with_idempotency_key(IdempotencyKey::new(q.operation_id.to_string())?);
    if let Some(minutes) = self_destruct {
        upload = upload.self_destructing(minutes);
    }
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

#[cfg(test)]
mod self_destruct_tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, body_string_contains, method, path, path_regex},
    };

    async fn upstream() -> (MockServer, App, HeaderMap) {
        let server = MockServer::start().await;
        let (app, headers) = session::tests::signed_in(&format!("{}/api/v1/", server.uri())).await;
        (server, app, headers)
    }
    fn entry(kind: &str, path: &str, self_destruct_at: Option<&str>) -> Value {
        json!({"id":Uuid::new_v4(),"org_id":"tos","type":kind,"visibility":"full",
            "name":path.rsplit('/').next(),"path":path,"parent_id":null,"root_type":"private",
            "tag":null,"content_type":null,"size":null,"render":null,
            "permanent_url":format!("https://briefcase.teamofsilicons.com/org/tos/{path}/"),
            "content_url":null,"download_url":null,"owner":null,"origin_app_id":null,
            "effective_access":["read","write","update","delete"],"created_at":null,
            "updated_at":null,"deleted_at":null,"self_destruct_at":self_destruct_at})
    }
    fn upload_request(headers: &HeaderMap, body: &'static str) -> Request {
        let mut request = Request::builder()
            .method("POST")
            .header(header::CONTENT_LENGTH, body.len())
            .body(Body::from(body))
            .unwrap();
        request.headers_mut().extend(headers.clone());
        request
    }
    fn query(minutes: Option<u32>) -> UploadQuery {
        UploadQuery {
            parent: "private/saket".into(),
            name: "note.txt".into(),
            content_type: "text/plain".into(),
            operation_id: Uuid::new_v4(),
            self_destruct_minutes: minutes,
        }
    }

    #[tokio::test]
    async fn keeping_a_file_stops_its_timer_and_explains_refusals() {
        let (server, app, headers) = upstream().await;
        let (kept, foreign, stopped) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for (id, response) in [
            (kept, ResponseTemplate::new(204)),
            (
                foreign,
                ResponseTemplate::new(403).set_body_json(json!({"error":{
                    "code":"forbidden","message":"The actor is not authorized for this action."}})),
            ),
            (
                stopped,
                ResponseTemplate::new(409).set_body_json(json!({"error":{
                    "code":"not_self_destructing",
                    "message":"The request conflicts with the current resource state."}})),
            ),
        ] {
            Mock::given(method("DELETE"))
                .and(path(format!("/api/v1/entries/{id}/self-destruct")))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
        }
        let result = keep(State(app.clone()), headers.clone(), Path(kept))
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(result.0, json!({"kept":true}));
        let Err(failure) = keep(State(app.clone()), headers.clone(), Path(foreign)).await else {
            panic!("kept another member's file");
        };
        assert_eq!(failure.0, StatusCode::FORBIDDEN);
        assert!(
            failure
                .1
                .starts_with("Only the person who uploaded this file")
        );
        let Err(failure) = keep(State(app), headers, Path(stopped)).await else {
            panic!("kept a permanent file");
        };
        assert_eq!(
            (failure.0, failure.1.as_str()),
            (
                StatusCode::CONFLICT,
                "This file is no longer set to self destruct."
            )
        );
    }

    #[tokio::test]
    async fn a_self_destructing_upload_sends_its_lifetime_upstream() {
        let (server, app, headers) = upstream().await;
        Mock::given(method("GET"))
            .and(path_regex("^/org/tos/private/saket/?$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry(
                "folder",
                "private/saket",
                None,
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("^/org/tos/private/saket/note.txt/?$"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error":{
                "code":"not_found","message":"The requested resource was not found."}})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/uploads"))
            .and(body_string_contains(
                "name=\"self_destruct_minutes\"\r\n\r\n90\r\n",
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(entry(
                "file",
                "private/saket/note.txt",
                Some("2026-09-24T13:30:00Z"),
            )))
            .expect(1)
            .mount(&server)
            .await;
        let uploaded = upload(
            State(app),
            Query(query(Some(90))),
            upload_request(&headers, "hi"),
        )
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(uploaded.0["self_destruct_at"], "2026-09-24T13:30:00Z");
    }

    #[tokio::test]
    async fn a_self_destructing_upload_over_an_existing_file_is_refused_before_transfer() {
        let (server, app, headers) = upstream().await;
        Mock::given(method("GET"))
            .and(path_regex("^/org/tos/private/saket/?$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry(
                "folder",
                "private/saket",
                None,
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex("^/org/tos/private/saket/note.txt/?$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(entry(
                "file",
                "private/saket/note.txt",
                None,
            )))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/uploads"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let Err(failure) = upload(
            State(app.clone()),
            Query(query(Some(90))),
            upload_request(&headers, "hi"),
        )
        .await
        else {
            panic!("uploaded a self-destructing version");
        };
        assert_eq!(failure.0, StatusCode::CONFLICT);
        assert!(
            failure
                .1
                .starts_with("Self destruct only applies to new files")
        );
        for minutes in [0, 43_201] {
            let Err(failure) = upload(
                State(app.clone()),
                Query(query(Some(minutes))),
                upload_request(&headers, "hi"),
            )
            .await
            else {
                panic!("accepted {minutes} minutes");
            };
            assert_eq!(failure.0, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn a_permission_grant_can_be_a_expiring_share() {
        let (server, app, headers) = upstream().await;
        let id = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/entries/{id}/permissions")))
            .and(body_json(
                json!({"principal":{"type":"carbon","id":"c:alex"},
                "access":["read"],"inherit":false,"expires_in_minutes":30}),
            ))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id":Uuid::new_v4(),"principal":{"type":"carbon","id":"c:alex"},
                "access":["read"],"inherit":false,
                "granted_by":{"type":"carbon","id":"saket"},
                "created_at":"2026-09-24T12:00:00Z","expires_at":"2026-09-24T12:30:00Z"})))
            .expect(1)
            .mount(&server)
            .await;
        let granted = grant(
            State(app),
            headers,
            Path(id),
            Json(
                serde_json::from_value(json!({"principal":{"type":"carbon","id":"c:alex"},
                    "access":["read"],"inherit":false,"expires_in_minutes":30}))
                .unwrap(),
            ),
        )
        .await
        .unwrap_or_else(|failure| panic!("{}", failure.1));
        assert_eq!(granted.0["expires_at"], "2026-09-24T12:30:00Z");
    }
}
