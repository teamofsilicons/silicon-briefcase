//! Organization upload allowances, counted where they are spent.
//!
//! An upload is checked twice. The check before the bytes are stored is
//! advisory: it refuses an upload that obviously does not fit, so a caller is
//! not asked to transfer bytes that cannot be kept. The charge at publication
//! is authoritative: it increments and re-reads the counters inside the same
//! transaction that records the file, so two uploads racing for the last of an
//! allowance serialize on that row and only one of them can win.

use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

use crate::{
    domain::quota::{UploadLimit, UploadUsage},
    error::AppError,
};

use super::metadata::common::map_sql;

/// The UTC day a charge belongs to, evaluated by PostgreSQL itself.
macro_rules! utc_today {
    () => {
        "(clock_timestamp() AT TIME ZONE 'UTC')::date"
    };
}

/// Refuses an upload that current usage already cannot admit.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the organization has spent
/// the allowance, or a database error.
pub(super) async fn check_allowance(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    let usage = read_usage(transaction).await?;
    usage.admits(bytes).map_err(exhausted)
}

/// Charges an upload against both allowances, refusing what does not fit.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the charge exceeds an
/// allowance, which rolls the whole publication back, or a database error.
pub(super) async fn charge(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    let charged = i64::try_from(bytes).map_err(|_| AppError::Internal {
        category: "upload_usage_overflow",
    })?;
    let (daily_bytes, total_bytes) = sqlx::query_as::<_, (i64, i64)>(concat!(
        "INSERT INTO briefcase.organization_upload_usage AS current_usage \
                (org_id, daily_window, daily_bytes, total_bytes) \
         VALUES (briefcase.current_org_id(), ",
        utc_today!(),
        ", $1, $1) \
         ON CONFLICT (org_id) DO UPDATE \
            SET daily_window = ",
        utc_today!(),
        ", daily_bytes = CASE \
                    WHEN current_usage.daily_window = ",
        utc_today!(),
        " THEN current_usage.daily_bytes + $1 \
                    ELSE $1 \
                END, \
                total_bytes = current_usage.total_bytes + $1 \
         RETURNING daily_bytes, total_bytes"
    ))
    .bind(charged)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    // The counters now include this upload, so admitting zero further bytes is
    // exactly the question "did this one still fit?".
    usage(daily_bytes, total_bytes).admits(0).map_err(exhausted)
}

async fn read_usage(transaction: &mut Transaction<'_, Postgres>) -> Result<UploadUsage, AppError> {
    let row = sqlx::query_as::<_, (i64, i64)>(concat!(
        "SELECT CASE WHEN daily_window = ",
        utc_today!(),
        " THEN daily_bytes ELSE 0 END, total_bytes \
           FROM briefcase.organization_upload_usage \
          WHERE org_id = briefcase.current_org_id()"
    ))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(
        row.map_or_else(UploadUsage::default, |(daily_bytes, total_bytes)| {
            usage(daily_bytes, total_bytes)
        }),
    )
}

fn usage(daily_bytes: i64, total_bytes: i64) -> UploadUsage {
    // The column is constrained non-negative, so a negative value could only
    // come from a corrupted row; treating it as spent is the safe reading.
    UploadUsage {
        daily_bytes: u64::try_from(daily_bytes).unwrap_or(u64::MAX),
        total_bytes: u64::try_from(total_bytes).unwrap_or(u64::MAX),
    }
}

fn exhausted(limit: UploadLimit) -> AppError {
    AppError::UploadLimitExhausted {
        limit,
        retry_after_seconds: limit.resets().then(seconds_until_utc_midnight),
    }
}

fn seconds_until_utc_midnight() -> u64 {
    let now = OffsetDateTime::now_utc();
    let Some(tomorrow) = now.date().next_day() else {
        return 1;
    };
    let seconds = (tomorrow.midnight().assume_utc() - now).whole_seconds();
    u64::try_from(seconds).unwrap_or(1).max(1)
}

fn database_error(error: sqlx::Error) -> AppError {
    match map_sql(error) {
        crate::application::service::MetadataRepositoryError::NotFound => AppError::NotFound,
        crate::application::service::MetadataRepositoryError::Conflict => {
            AppError::conflict("upload_usage_conflict")
        }
        _ => AppError::Internal {
            category: "upload_usage",
        },
    }
}
