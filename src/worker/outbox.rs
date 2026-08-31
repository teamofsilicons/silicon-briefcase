//! Cross-tenant outbox leasing and failure-honest dispatch.

use sqlx::PgPool;
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::WorkerSettings;

use super::policy::{classify_topic, retry_delay, should_dead_letter};

#[derive(Debug, sqlx::FromRow)]
struct ClaimedEvent {
    org_id: String,
    event_id: Uuid,
    topic: String,
    attempt_count: i32,
}

#[derive(Debug, Error)]
pub(super) enum BatchError {
    #[error("outbox database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("outbox retry delay cannot be represented by PostgreSQL")]
    RetryDelayNotRepresentable,
}

impl BatchError {
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::Database(_) => "outbox_database_error",
            Self::RetryDelayNotRepresentable => "outbox_retry_delay_invalid",
        }
    }
}

pub(super) async fn process_batch(
    pool: &PgPool,
    settings: &WorkerSettings,
    batch_size: i64,
    lease_duration_millis: i64,
) -> Result<(), BatchError> {
    let lease_token = Uuid::now_v7();
    let events = claim(pool, batch_size, lease_token, lease_duration_millis).await?;

    for event in events {
        process_event(pool, settings, lease_token, &event).await?;
    }
    Ok(())
}

async fn claim(
    pool: &PgPool,
    batch_size: i64,
    lease_token: Uuid,
    lease_duration_millis: i64,
) -> Result<Vec<ClaimedEvent>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let events = sqlx::query_as::<_, ClaimedEvent>(
        "WITH candidates AS ( \
             SELECT org_id, event_id \
               FROM briefcase.outbox_events \
              WHERE (status = 'pending' AND available_at <= clock_timestamp()) \
                 OR (status = 'processing' AND lease_expires_at <= clock_timestamp()) \
              ORDER BY available_at, created_at, org_id, event_id \
              FOR UPDATE SKIP LOCKED \
              LIMIT $1 \
         ) \
         UPDATE briefcase.outbox_events AS event \
            SET status = 'processing', \
                attempt_count = event.attempt_count + 1, \
                lease_token = $2, \
                lease_expires_at = clock_timestamp() \
                    + ($3::bigint * interval '1 millisecond') \
           FROM candidates \
          WHERE event.org_id = candidates.org_id \
            AND event.event_id = candidates.event_id \
         RETURNING event.org_id, event.event_id, event.topic, event.attempt_count",
    )
    .bind(batch_size)
    .bind(lease_token)
    .bind(lease_duration_millis)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(events)
}

async fn process_event(
    pool: &PgPool,
    settings: &WorkerSettings,
    lease_token: Uuid,
    event: &ClaimedEvent,
) -> Result<(), BatchError> {
    let failure = classify_topic(&event.topic);
    let attempt = match u16::try_from(event.attempt_count) {
        Ok(attempt) => attempt,
        Err(_) => settings.max_attempts,
    };
    let terminal = should_dead_letter(attempt, settings.max_attempts);
    let delay = retry_delay(
        settings.poll_interval,
        settings.max_retry_delay,
        attempt,
        event.event_id,
    );
    let retry_delay_millis = retry_delay_millis(delay)?;

    let released = release_failure(
        pool,
        event,
        lease_token,
        retry_delay_millis,
        failure.code(),
        terminal,
    )
    .await?;

    if !released {
        warn!(
            event = "outbox_lease_lost",
            event_id = %event.event_id,
            "outbox result ignored after lease loss"
        );
    } else if terminal {
        warn!(
            event = "outbox_event_dead_lettered",
            event_id = %event.event_id,
            attempt,
            error_code = failure.code(),
            "outbox event reached its terminal attempt"
        );
    } else {
        info!(
            event = "outbox_event_retry_scheduled",
            event_id = %event.event_id,
            attempt,
            error_code = failure.code(),
            "outbox event scheduled for retry"
        );
    }
    Ok(())
}

fn retry_delay_millis(delay: std::time::Duration) -> Result<i64, BatchError> {
    let has_submillisecond_remainder = !delay.subsec_nanos().is_multiple_of(1_000_000);
    let rounded = delay
        .as_millis()
        .saturating_add(u128::from(has_submillisecond_remainder))
        .max(1);
    i64::try_from(rounded).map_err(|_| BatchError::RetryDelayNotRepresentable)
}

async fn release_failure(
    pool: &PgPool,
    event: &ClaimedEvent,
    lease_token: Uuid,
    retry_delay_millis: i64,
    error_code: &'static str,
    terminal: bool,
) -> Result<bool, sqlx::Error> {
    let status = if terminal { "dead_letter" } else { "pending" };
    let mut transaction = pool.begin().await?;
    let result = sqlx::query(
        "UPDATE briefcase.outbox_events \
            SET status = $4, \
                available_at = CASE \
                    WHEN $4 = 'pending' THEN clock_timestamp() \
                        + ($5::bigint * interval '1 millisecond') \
                    ELSE available_at \
                END, \
                lease_token = NULL, \
                lease_expires_at = NULL, \
                last_error = $6 \
          WHERE org_id = $1 \
            AND event_id = $2 \
            AND status = 'processing' \
            AND lease_token = $3",
    )
    .bind(&event.org_id)
    .bind(event.event_id)
    .bind(lease_token)
    .bind(status)
    .bind(retry_delay_millis)
    .bind(error_code)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::{super::policy::retry_delay, retry_delay_millis};

    #[test]
    fn retry_delay_remains_positive_for_valid_worker_settings() {
        let delay = retry_delay(
            Duration::from_millis(500),
            Duration::from_secs(300),
            1,
            Uuid::from_u128(7),
        );
        assert!(delay > Duration::ZERO);
    }

    #[test]
    fn database_retry_delay_rounds_up_to_a_positive_millisecond() {
        assert!(matches!(retry_delay_millis(Duration::from_nanos(1)), Ok(1)));
        assert!(matches!(
            retry_delay_millis(Duration::from_millis(250)),
            Ok(250)
        ));
    }
}
