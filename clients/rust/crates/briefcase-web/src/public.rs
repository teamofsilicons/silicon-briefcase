//! Anonymous links always use an unauthenticated official Briefcase client.
use crate::{App, Result, bad};
use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicQuery {
    org: String,
    path: String,
    view: Option<String>,
    cursor: Option<String>,
}

pub(crate) async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Query(q): Query<PublicQuery>,
) -> Result<Response> {
    let client = briefcase_client::Client::new_unchecked(
        briefcase_client::Config::for_sign_in(&app.upstream)?.with_auto_update(false),
    )?;
    let mut response = match q.view.as_deref().unwrap_or("metadata") {
        "metadata" => Json(client.public_entry(&q.org, &q.path).await?).into_response(),
        "contents" => Json(
            client
                .public_children(&q.org, &q.path, q.cursor.as_deref())
                .await?,
        )
        .into_response(),
        view @ ("inline" | "attachment") => {
            let entry = client.public_entry(&q.org, &q.path).await?;
            if entry.entry_type == "folder" && view == "inline" {
                return Err(bad("Choose a file to preview."));
            }
            let range = crate::files::requested_range(&headers, entry.size.unwrap_or(0))?;
            let stream = if view == "attachment" && range.is_none() {
                client.public_download(&q.org, &q.path).await?
            } else {
                client.public_content(&q.org, &q.path, range).await?
            };
            let length = stream.content_length();
            let content_range = stream.content_range().map(str::to_owned);
            let mime = stream
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_owned();
            let stream = futures_util::stream::try_unfold(stream, |mut stream| async move {
                stream
                    .chunk()
                    .await
                    .map(|chunk| chunk.map(|chunk| (chunk, stream)))
                    .map_err(std::io::Error::other)
            });
            let mut response = Response::new(Body::from_stream(stream));
            if let Some(length) = length {
                response
                    .headers_mut()
                    .insert(header::CONTENT_LENGTH, HeaderValue::from(length));
            }
            if let Some(range) = content_range {
                *response.status_mut() = StatusCode::PARTIAL_CONTENT;
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&range).map_err(|_| bad("Invalid byte range"))?,
                );
            }
            if entry.entry_type == "file" {
                response
                    .headers_mut()
                    .insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            }
            let filename = if entry.entry_type == "folder" {
                format!("{}.tar.zst", entry.name)
            } else {
                entry.name
            };
            let filename: String =
                url::form_urlencoded::byte_serialize(filename.as_bytes()).collect();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(&mime).map_err(|_| bad("Invalid media type"))?,
            );
            response.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!(
                    "{view}; filename*=UTF-8''{}",
                    filename.replace('+', "%20")
                ))
                .map_err(|_| bad("Invalid filename"))?,
            );
            response.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("sandbox; default-src 'none'; frame-ancestors 'self'"),
            );
            response
        }
        _ => return Err(bad("Invalid public view")),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
