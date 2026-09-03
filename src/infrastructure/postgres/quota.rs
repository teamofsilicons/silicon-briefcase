//! Organization usage counters, read and charged where they are spent.
//!
//! An upload is measured twice. Before the bytes are asked for, the check is
//! advisory: it refuses an upload that obviously cannot be kept, so a caller is
//! not made to transfer a file that has nowhere to go. At publication the
//! charge is authoritative: it locks the organization's counter row inside the
//! transaction that records the file, so two uploads racing for the last of an
//! allowance serialize and only one of them can win.
//!
//! Stored bytes are not charged here at all. A database trigger moves that
//! counter with the version rows themselves, so publication, retention, bin
//! purges, and cascading deletes all account for storage without knowing it.

use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;

use crate::{
    domain::quota::{OrganizationUsage, UploadLimit},
    error::AppError,
};

use super::metadata::common::map_sql;

/// The UTC day a charge belongs to, evaluated by PostgreSQL itself.
macro_rules! utc_today {
    () => {
        "(clock_timestamp() AT TIME ZONE 'UTC')::date"
    };
}

/// Reads what the organization currently consumes and the ceilings in force.
///
/// # Errors
///
/// Returns an error when the usage row cannot be read.
pub(super) async fn read_usage(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<OrganizationUsage, AppError> {
    let row = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>)>(concat!(
        "SELECT CASE WHEN daily_window = ",
        utc_today!(),
        " THEN daily_upload_bytes ELSE 0 END, \
           stored_bytes, daily_upload_limit_bytes, storage_limit_bytes \
           FROM briefcase.organization_usage \
          WHERE org_id = briefcase.current_org_id()"
    ))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(row.map_or(OrganizationUsage::EMPTY, usage))
}

/// Refuses an upload that current usage already cannot admit.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the organization has no room
/// for the upload, or a database error.
pub(super) async fn check_upload(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    read_usage(transaction)
        .await?
        .admits_upload(bytes)
        .map_err(exhausted)
}

/// Refuses bytes the organization has no room to store.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the storage ceiling is
/// reached, or a database error.
pub(super) async fn check_storage(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    read_usage(transaction)
        .await?
        .admits_storage(bytes)
        .map_err(exhausted)
}

/// Charges an upload against the daily allowance and the storage ceiling.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the upload exceeds a limit,
/// or a database error.
pub(super) async fn charge_upload(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    let usage = lock_usage(transaction, bytes).await?;
    // The daily counter now includes this upload while the stored counter does
    // not: the version row that moves it is inserted next. Asking whether the
    // organization still admits these bytes therefore answers both questions
    // at once — did the day have room, and does the storage.
    usage
        .admits_upload(0)
        .and_then(|()| usage.admits_storage(bytes))
        .map_err(exhausted)
}

/// Claims storage for bytes that are copied rather than uploaded.
///
/// A restore consumes storage without spending the day's upload allowance, so
/// it locks the same row and answers to the storage ceiling alone.
///
/// # Errors
///
/// Returns [`AppError::UploadLimitExhausted`] when the storage ceiling is
/// reached, or a database error.
pub(super) async fn charge_storage(
    transaction: &mut Transaction<'_, Postgres>,
    bytes: u64,
) -> Result<(), AppError> {
    lock_usage(transaction, 0)
        .await?
        .admits_storage(bytes)
        .map_err(exhausted)
}

/// Adds `daily_charge` to the day's counter and returns the locked row.
///
/// The upsert locks the organization's row before the caller reads it, so a
/// concurrent write cannot slip past on stale counters. Returning an error
/// rolls the whole publication back, charging nothing.
async fn lock_usage(
    transaction: &mut Transaction<'_, Postgres>,
    daily_charge: u64,
) -> Result<OrganizationUsage, AppError> {
    let charged = i64::try_from(daily_charge).map_err(|_| AppError::Internal {
        category: "usage_overflow",
    })?;
    let row = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>)>(concat!(
        "INSERT INTO briefcase.organization_usage AS usage_row \
                (org_id, daily_window, daily_upload_bytes) \
         VALUES (briefcase.current_org_id(), ",
        utc_today!(),
        ", $1) \
         ON CONFLICT (org_id) DO UPDATE \
            SET daily_window = ",
        utc_today!(),
        ", daily_upload_bytes = CASE \
                    WHEN usage_row.daily_window = ",
        utc_today!(),
        " THEN usage_row.daily_upload_bytes + $1 \
                    ELSE $1 \
                END \
         RETURNING daily_upload_bytes, stored_bytes, \
                   daily_upload_limit_bytes, storage_limit_bytes"
    ))
    .bind(charged)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)?;
    Ok(usage(row))
}

fn usage(
    (daily_upload_bytes, stored_bytes, daily_upload_limit, storage_limit): (
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ),
) -> OrganizationUsage {
    OrganizationUsage {
        daily_upload_bytes: counter(daily_upload_bytes),
        stored_bytes: counter(stored_bytes),
        daily_upload_limit: daily_upload_limit.map(counter),
        storage_limit: storage_limit.map(counter),
    }
}

/// Reads one non-negative counter or ceiling.
///
/// The columns are constrained non-negative, so a negative value could only
/// come from a corrupted row; reading it as exhausted is the safe direction.
fn counter(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
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
            AppError::conflict("organization_usage_conflict")
        }
        _ => AppError::Internal {
            category: "organization_usage",
        },
    }
}
