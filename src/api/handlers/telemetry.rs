//! Anonymous operational telemetry, never an authority-bearing endpoint.
use crate::error::AppError;
use axum::{Json, http::StatusCode};
use briefcase_client::telemetry::{Event, Source};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

static ACCEPTED: LazyLock<Mutex<HashMap<uuid::Uuid, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static RATE: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

pub(crate) async fn submit(Json(event): Json<Event>) -> Result<StatusCode, AppError> {
    if !event.valid() || matches!(event.source, Source::Backend | Source::Worker) {
        return Err(AppError::validation("invalid_telemetry_event"));
    }
    {
        let mut rate = RATE.lock().map_err(|_| AppError::Internal {
            category: "telemetry_rate_lock",
        })?;
        let (started, count) = rate.get_or_insert((Instant::now(), 0));
        if started.elapsed() >= Duration::from_secs(1) {
            *started = Instant::now();
            *count = 0;
        }
        if *count >= 100 {
            return Ok(StatusCode::TOO_MANY_REQUESTS);
        }
        *count += 1;
    }
    {
        let mut accepted = ACCEPTED.lock().map_err(|_| AppError::Internal {
            category: "telemetry_replay_lock",
        })?;
        accepted.retain(|_, at| at.elapsed() < Duration::from_secs(60));
        if accepted.contains_key(&event.id) {
            return Ok(StatusCode::NO_CONTENT);
        }
        // The rate limit bounds this map to at most 6,100 entries.
        accepted.insert(event.id, Instant::now());
    }
    // The telemetry endpoint is deliberately excluded from request telemetry.
    // Explicit reports are observed as client data, never trusted actor facts.
    crate::telemetry::record(event, true);
    Ok(StatusCode::NO_CONTENT)
}
