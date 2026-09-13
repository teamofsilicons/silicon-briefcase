//! Same-origin, content-free browser telemetry. No table keys reach JavaScript.
use crate::{App, Result, bad};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use briefcase_client::telemetry::{Event, Source};
use serde::Deserialize;

pub(crate) fn enabled(headers: &HeaderMap) -> bool {
    briefcase_client::telemetry::enabled_from_env()
        && !headers
            .get("x-briefcase-telemetry")
            .is_some_and(|value| value == "off")
        && !headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .any(|value| value.trim() == "briefcase_telemetry=off")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Batch {
    table: String,
    events: Vec<Event>,
}

pub(crate) async fn submit(
    State(app): State<App>,
    headers: HeaderMap,
    Json(batch): Json<Batch>,
) -> Result<StatusCode> {
    if !enabled(&headers) {
        return Ok(StatusCode::NO_CONTENT);
    }
    if batch.table != "siliconbriefcase"
        || batch.events.len() > 40
        || batch
            .events
            .iter()
            .any(|event| !event.valid() || event.source != Source::Web)
    {
        return Err(bad("Invalid telemetry batch."));
    }
    use futures_util::{StreamExt as _, TryStreamExt as _};
    futures_util::stream::iter(batch.events.into_iter().map(|event| {
        let upstream = app.upstream.clone();
        async move { briefcase_client::telemetry::submit(&upstream, &event).await }
    }))
    .buffer_unordered(4)
    .try_collect::<Vec<_>>()
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
