//! Durable provider cleanup and metadata retention.

use std::time::Duration;

use futures::{StreamExt as _, stream};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    application::ports::{ObjectKey, ObjectStore, ObjectStoreError, StorageTarget},
    domain::storage::EncryptionMode,
    infrastructure::s3::organization_storage_external_id,
};

use super::policy::retry_delay;

const RETAINED_VERSION_COUNT: i64 = 50;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CleanupStats {
    pub(super) multipart_jobs_scheduled: u64,
    pub(super) version_jobs_scheduled: u64,
    pub(super) provider_operations_completed: u64,
    pub(super) provider_operations_retried: u64,
    pub(super) deletion_batches_purged: u64,
}

#[derive(Debug, Error)]
pub(super) enum CleanupBatchError {
    #[error("cleanup database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("cleanup duration cannot be represented by PostgreSQL")]
    DurationNotRepresentable,
}

impl CleanupBatchError {
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::Database(_) => "cleanup_database_error",
            Self::DurationNotRepresentable => "cleanup_duration_invalid",
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct CleanupSource {
    org_id: String,
    source_entry_id: Option<Uuid>,
    source_version_id: Option<Uuid>,
    source_upload_id: Option<Uuid>,
    deletion_batch_id: Option<Uuid>,
    storage_backend: String,
    storage_config_id: Option<Uuid>,
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
    provider_upload_id: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct ClaimedCleanup {
    org_id: String,
    cleanup_id: Uuid,
    cleanup_kind: String,
    source_entry_id: Option<Uuid>,
    source_version_id: Option<Uuid>,
    source_upload_id: Option<Uuid>,
    storage_backend: String,
    storage_config_id: Option<Uuid>,
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
    provider_upload_id: Option<String>,
    attempt_count: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CleanupKind {
    MultipartAbort,
    VersionDelete,
}

impl CleanupKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::MultipartAbort => "multipart_abort",
            Self::VersionDelete => "version_delete",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "multipart_abort" => Some(Self::MultipartAbort),
            "version_delete" => Some(Self::VersionDelete),
            _ => None,
        }
    }
}

enum Preflight {
    Execute,
    Cancel,
}

/// Schedules bounded cleanup work, processes at most one configured batch, and
/// permanently retires deletion batches whose exact objects are all gone.
pub(super) async fn process_batch<O>(
    pool: &PgPool,
    objects: &O,
    batch_size: i64,
    concurrency: usize,
    lease_duration_millis: i64,
    retry_base: Duration,
    max_retry_delay: Duration,
) -> Result<CleanupStats, CleanupBatchError>
where
    O: ObjectStore + ?Sized,
{
    let mut stats = CleanupStats {
        multipart_jobs_scheduled: schedule_multipart_aborts(pool, batch_size).await?,
        version_jobs_scheduled: schedule_version_deletions(pool, batch_size).await?,
        ..CleanupStats::default()
    };

    let mut operations = stream::iter(0..batch_size)
        .map(|_| async {
            let lease_token = Uuid::now_v7();
            let Some(job) = claim_one(pool, lease_token, lease_duration_millis).await? else {
                return Ok::<Option<ProcessOutcome>, CleanupBatchError>(None);
            };
            process_claimed(
                pool,
                objects,
                &job,
                lease_token,
                retry_base,
                max_retry_delay,
            )
            .await
            .map(Some)
        })
        .buffer_unordered(concurrency);
    while let Some(outcome) = operations.next().await {
        match outcome? {
            Some(ProcessOutcome::Completed) => stats.provider_operations_completed += 1,
            Some(ProcessOutcome::Retried) => stats.provider_operations_retried += 1,
            Some(ProcessOutcome::Cancelled) | None => {}
        }
    }

    stats.deletion_batches_purged = finalize_deletion_batches(pool, batch_size).await?;
    Ok(stats)
}

async fn schedule_multipart_aborts(pool: &PgPool, batch_size: i64) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let sources = sqlx::query_as::<_, CleanupSource>(
        "SELECT upload.org_id, NULL::uuid AS source_entry_id, \
                NULL::uuid AS source_version_id, upload.upload_id AS source_upload_id, \
                NULL::uuid AS deletion_batch_id, upload.storage_backend, \
                upload.storage_config_id, upload.bucket_name, upload.storage_region, \
                upload.storage_prefix, configuration.role_arn AS storage_role_arn, \
                upload.storage_encryption_mode, upload.storage_kms_key_arn, \
                upload.object_key, NULL::text AS object_version_id, \
                upload.provider_upload_id \
           FROM briefcase.multipart_uploads AS upload \
           LEFT JOIN briefcase.organization_storage_configs AS configuration \
             ON configuration.org_id = upload.org_id \
            AND configuration.storage_config_id = upload.storage_config_id \
          WHERE ( \
                    (upload.status IN ('initiated', 'uploading') \
                        AND upload.expires_at <= clock_timestamp()) \
                    OR upload.status IN ('aborted', 'expired') \
                ) \
            AND NOT EXISTS ( \
                SELECT 1 FROM briefcase.object_cleanup_jobs AS cleanup \
                 WHERE cleanup.org_id = upload.org_id \
                   AND cleanup.cleanup_kind = 'multipart_abort' \
                   AND cleanup.source_upload_id = upload.upload_id \
            ) \
          ORDER BY upload.expires_at, upload.org_id, upload.upload_id \
          FOR UPDATE OF upload SKIP LOCKED \
          LIMIT $1",
    )
    .bind(batch_size)
    .fetch_all(&mut *transaction)
    .await?;

    let mut scheduled = 0_u64;
    for source in sources {
        sqlx::query(
            "UPDATE briefcase.multipart_uploads \
                SET status = 'expired' \
              WHERE org_id = $1 AND upload_id = $2 \
                AND status IN ('initiated', 'uploading') \
                AND expires_at <= clock_timestamp()",
        )
        .bind(&source.org_id)
        .bind(source.source_upload_id)
        .execute(&mut *transaction)
        .await?;
        scheduled +=
            insert_cleanup_job(&mut transaction, CleanupKind::MultipartAbort, &source).await?;
    }
    transaction.commit().await?;
    Ok(scheduled)
}

async fn schedule_version_deletions(pool: &PgPool, batch_size: i64) -> Result<u64, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let sources = sqlx::query_as::<_, CleanupSource>(
        "WITH ranked_versions AS MATERIALIZED ( \
             SELECT version.org_id, version.entry_id, version.version_id, \
                    row_number() OVER ( \
                        PARTITION BY version.org_id, version.entry_id \
                        ORDER BY version.version_number DESC, version.version_id DESC \
                    ) AS retention_rank \
               FROM briefcase.entry_versions AS version \
         ) \
         SELECT version.org_id, version.entry_id AS source_entry_id, \
                version.version_id AS source_version_id, \
                NULL::uuid AS source_upload_id, entry.deletion_batch_id, \
                version.storage_backend, version.storage_config_id, version.bucket_name, \
                version.storage_region, version.storage_prefix, \
                configuration.role_arn AS storage_role_arn, \
                version.storage_encryption_mode, version.storage_kms_key_arn, \
                version.object_key, version.object_version_id, \
                NULL::text AS provider_upload_id \
           FROM ranked_versions AS ranked \
           JOIN briefcase.entry_versions AS version \
             ON version.org_id = ranked.org_id \
            AND version.entry_id = ranked.entry_id \
            AND version.version_id = ranked.version_id \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = version.org_id \
            AND entry.entry_id = version.entry_id \
           LEFT JOIN briefcase.organization_storage_configs AS configuration \
             ON configuration.org_id = version.org_id \
            AND configuration.storage_config_id = version.storage_config_id \
          WHERE ( \
                    (entry.deleted_at IS NOT NULL \
                        AND entry.purge_after <= clock_timestamp()) \
                    OR (entry.deleted_at IS NULL \
                        AND ranked.retention_rank > $2 \
                        AND entry.current_version_id <> version.version_id) \
                ) \
            AND NOT EXISTS ( \
                SELECT 1 FROM briefcase.object_cleanup_jobs AS cleanup \
                 WHERE cleanup.org_id = version.org_id \
                   AND cleanup.cleanup_kind = 'version_delete' \
                   AND cleanup.source_entry_id = version.entry_id \
                   AND cleanup.source_version_id = version.version_id \
            ) \
          ORDER BY (entry.deleted_at IS NULL), entry.purge_after, \
                   version.created_at, version.org_id, version.entry_id, version.version_id \
          FOR UPDATE OF version, entry SKIP LOCKED \
          LIMIT $1",
    )
    .bind(batch_size)
    .bind(RETAINED_VERSION_COUNT)
    .fetch_all(&mut *transaction)
    .await?;

    let mut scheduled = 0_u64;
    for source in sources {
        scheduled +=
            insert_cleanup_job(&mut transaction, CleanupKind::VersionDelete, &source).await?;
    }
    transaction.commit().await?;
    Ok(scheduled)
}

async fn insert_cleanup_job(
    transaction: &mut Transaction<'_, Postgres>,
    kind: CleanupKind,
    source: &CleanupSource,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO briefcase.object_cleanup_jobs ( \
                org_id, cleanup_id, cleanup_kind, source_entry_id, source_version_id, \
                source_upload_id, deletion_batch_id, storage_backend, storage_config_id, \
                bucket_name, storage_region, storage_prefix, storage_role_arn, \
                storage_encryption_mode, storage_kms_key_arn, object_key, \
                object_version_id, provider_upload_id \
         ) VALUES ( \
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, \
                $16, $17, $18 \
         ) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&source.org_id)
    .bind(Uuid::now_v7())
    .bind(kind.as_str())
    .bind(source.source_entry_id)
    .bind(source.source_version_id)
    .bind(source.source_upload_id)
    .bind(source.deletion_batch_id)
    .bind(&source.storage_backend)
    .bind(source.storage_config_id)
    .bind(&source.bucket_name)
    .bind(&source.storage_region)
    .bind(&source.storage_prefix)
    .bind(source.storage_role_arn.as_deref())
    .bind(&source.storage_encryption_mode)
    .bind(source.storage_kms_key_arn.as_deref())
    .bind(&source.object_key)
    .bind(source.object_version_id.as_deref())
    .bind(source.provider_upload_id.as_deref())
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected())
}

async fn claim_one(
    pool: &PgPool,
    lease_token: Uuid,
    lease_duration_millis: i64,
) -> Result<Option<ClaimedCleanup>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let job = sqlx::query_as::<_, ClaimedCleanup>(
        "WITH candidate AS ( \
             SELECT org_id, cleanup_id \
               FROM briefcase.object_cleanup_jobs \
              WHERE (status = 'pending' AND available_at <= clock_timestamp()) \
                 OR (status = 'processing' AND lease_expires_at <= clock_timestamp()) \
              ORDER BY available_at, created_at, org_id, cleanup_id \
              FOR UPDATE SKIP LOCKED \
              LIMIT 1 \
         ) \
         UPDATE briefcase.object_cleanup_jobs AS cleanup \
            SET status = 'processing', \
                attempt_count = CASE \
                    WHEN cleanup.attempt_count < 2147483647 \
                        THEN cleanup.attempt_count + 1 \
                    ELSE cleanup.attempt_count \
                END, \
                lease_token = $1, \
                lease_expires_at = clock_timestamp() \
                    + ($2::bigint * interval '1 millisecond') \
           FROM candidate \
          WHERE cleanup.org_id = candidate.org_id \
            AND cleanup.cleanup_id = candidate.cleanup_id \
         RETURNING cleanup.org_id, cleanup.cleanup_id, cleanup.cleanup_kind, \
                   cleanup.source_entry_id, cleanup.source_version_id, \
                   cleanup.source_upload_id, cleanup.storage_backend, \
                   cleanup.storage_config_id, cleanup.bucket_name, cleanup.storage_region, \
                   cleanup.storage_prefix, cleanup.storage_role_arn, \
                   cleanup.storage_encryption_mode, cleanup.storage_kms_key_arn, \
                   cleanup.object_key, cleanup.object_version_id, \
                   cleanup.provider_upload_id, cleanup.attempt_count",
    )
    .bind(lease_token)
    .bind(lease_duration_millis)
    .fetch_optional(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(job)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessOutcome {
    Completed,
    Retried,
    Cancelled,
}

async fn process_claimed<O>(
    pool: &PgPool,
    objects: &O,
    job: &ClaimedCleanup,
    lease_token: Uuid,
    retry_base: Duration,
    max_retry_delay: Duration,
) -> Result<ProcessOutcome, CleanupBatchError>
where
    O: ObjectStore + ?Sized,
{
    let Some(kind) = CleanupKind::parse(&job.cleanup_kind) else {
        return retry_claim(
            pool,
            job,
            lease_token,
            retry_base,
            max_retry_delay,
            "cleanup_descriptor_invalid",
        )
        .await;
    };
    if matches!(preflight(pool, job, kind).await?, Preflight::Cancel) {
        cancel_claim(pool, job, lease_token).await?;
        return Ok(ProcessOutcome::Cancelled);
    }
    let (target, key) = match cleanup_target(job) {
        Ok(target) => target,
        Err(code) => {
            return retry_claim(pool, job, lease_token, retry_base, max_retry_delay, code).await;
        }
    };

    let provider_result = match kind {
        CleanupKind::MultipartAbort => {
            let Some(provider_upload_id) = job.provider_upload_id.as_deref() else {
                return retry_claim(
                    pool,
                    job,
                    lease_token,
                    retry_base,
                    max_retry_delay,
                    "cleanup_descriptor_invalid",
                )
                .await;
            };
            objects
                .abort_multipart(&target, &key, provider_upload_id)
                .await
        }
        CleanupKind::VersionDelete => {
            objects
                .delete(&target, &key, job.object_version_id.as_deref())
                .await
        }
    };

    match provider_result {
        Ok(()) | Err(ObjectStoreError::NotFound) => {
            let settled = settle_success(pool, job, kind, lease_token).await?;
            if settled {
                info!(
                    event = "object_cleanup_completed",
                    cleanup_id = %job.cleanup_id,
                    cleanup_kind = kind.as_str(),
                    attempt = job.attempt_count,
                    "provider cleanup completed"
                );
            } else {
                warn!(
                    event = "object_cleanup_lease_lost",
                    cleanup_id = %job.cleanup_id,
                    cleanup_kind = kind.as_str(),
                    "cleanup result ignored after lease loss"
                );
            }
            Ok(ProcessOutcome::Completed)
        }
        Err(error) => {
            let code = provider_error_code(&error);
            retry_claim(pool, job, lease_token, retry_base, max_retry_delay, code).await
        }
    }
}

async fn preflight(
    pool: &PgPool,
    job: &ClaimedCleanup,
    kind: CleanupKind,
) -> Result<Preflight, sqlx::Error> {
    match kind {
        CleanupKind::MultipartAbort => {
            let Some(upload_id) = job.source_upload_id else {
                return Ok(Preflight::Cancel);
            };
            let status = sqlx::query_scalar::<_, String>(
                "SELECT status FROM briefcase.multipart_uploads \
                  WHERE org_id = $1 AND upload_id = $2",
            )
            .bind(&job.org_id)
            .bind(upload_id)
            .fetch_optional(pool)
            .await?;
            if matches!(status.as_deref(), Some("aborted" | "expired") | None) {
                Ok(Preflight::Execute)
            } else {
                Ok(Preflight::Cancel)
            }
        }
        CleanupKind::VersionDelete => {
            let (Some(entry_id), Some(version_id)) = (job.source_entry_id, job.source_version_id)
            else {
                return Ok(Preflight::Cancel);
            };
            let eligible = sqlx::query_scalar::<_, bool>(
                "SELECT CASE \
                            WHEN entry.deleted_at IS NOT NULL \
                                THEN entry.purge_after <= clock_timestamp() \
                            ELSE entry.current_version_id <> version.version_id \
                                AND ( \
                                    SELECT count(*) \
                                      FROM briefcase.entry_versions AS newer \
                                     WHERE newer.org_id = version.org_id \
                                       AND newer.entry_id = version.entry_id \
                                       AND newer.version_number > version.version_number \
                                ) >= $4 \
                        END \
                   FROM briefcase.entry_versions AS version \
                   JOIN briefcase.entries AS entry \
                     ON entry.org_id = version.org_id \
                    AND entry.entry_id = version.entry_id \
                  WHERE version.org_id = $1 \
                    AND version.entry_id = $2 \
                    AND version.version_id = $3",
            )
            .bind(&job.org_id)
            .bind(entry_id)
            .bind(version_id)
            .bind(RETAINED_VERSION_COUNT)
            .fetch_optional(pool)
            .await?;
            match eligible {
                Some(true) | None => Ok(Preflight::Execute),
                Some(false) => Ok(Preflight::Cancel),
            }
        }
    }
}

fn cleanup_target(job: &ClaimedCleanup) -> Result<(StorageTarget, ObjectKey), &'static str> {
    let encryption = match job.storage_encryption_mode.as_str() {
        "sse_s3" if job.storage_kms_key_arn.is_none() => EncryptionMode::SseS3,
        "sse_kms" if job.storage_kms_key_arn.is_some() => EncryptionMode::SseKms,
        _ => return Err("cleanup_descriptor_invalid"),
    };
    let (role_arn, external_id) = match job.storage_backend.as_str() {
        "platform" if job.storage_config_id.is_none() && job.storage_role_arn.is_none() => {
            (None, None)
        }
        "organization" if job.storage_config_id.is_some() => {
            let role_arn = job
                .storage_role_arn
                .clone()
                .ok_or("cleanup_descriptor_invalid")?;
            (
                Some(role_arn),
                Some(organization_storage_external_id(&job.org_id)),
            )
        }
        _ => return Err("cleanup_descriptor_invalid"),
    };
    let key = ObjectKey::new(job.object_key.clone()).map_err(|_| "cleanup_descriptor_invalid")?;
    Ok((
        StorageTarget {
            bucket: job.bucket_name.clone(),
            region: job.storage_region.clone(),
            prefix: job.storage_prefix.clone(),
            role_arn,
            external_id,
            encryption,
            kms_key_arn: job.storage_kms_key_arn.clone(),
        },
        key,
    ))
}

async fn settle_success(
    pool: &PgPool,
    job: &ClaimedCleanup,
    kind: CleanupKind,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    match kind {
        CleanupKind::MultipartAbort => settle_multipart(pool, job, lease_token).await,
        CleanupKind::VersionDelete => settle_version(pool, job, lease_token).await,
    }
}

async fn settle_multipart(
    pool: &PgPool,
    job: &ClaimedCleanup,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let held = lock_claim(&mut transaction, job, lease_token).await?;
    if !held {
        transaction.commit().await?;
        return Ok(false);
    }
    if let Some(upload_id) = job.source_upload_id {
        sqlx::query(
            "DELETE FROM briefcase.multipart_uploads \
              WHERE org_id = $1 AND upload_id = $2 \
                AND status IN ('aborted', 'expired')",
        )
        .bind(&job.org_id)
        .bind(upload_id)
        .execute(&mut *transaction)
        .await?;
    }
    delete_claim(&mut transaction, job, lease_token).await?;
    transaction.commit().await?;
    Ok(true)
}

#[derive(Debug, sqlx::FromRow)]
struct VersionState {
    current_version_id: Option<Uuid>,
    deletion_batch_id: Option<Uuid>,
    deleted_at: Option<time::OffsetDateTime>,
}

async fn settle_version(
    pool: &PgPool,
    job: &ClaimedCleanup,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let held = lock_claim(&mut transaction, job, lease_token).await?;
    if !held {
        transaction.commit().await?;
        return Ok(false);
    }
    let (Some(entry_id), Some(version_id)) = (job.source_entry_id, job.source_version_id) else {
        delete_claim(&mut transaction, job, lease_token).await?;
        transaction.commit().await?;
        return Ok(true);
    };
    let state = sqlx::query_as::<_, VersionState>(
        "SELECT entry.current_version_id, entry.deletion_batch_id, entry.deleted_at \
           FROM briefcase.entry_versions AS version \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = version.org_id \
            AND entry.entry_id = version.entry_id \
          WHERE version.org_id = $1 \
            AND version.entry_id = $2 \
            AND version.version_id = $3 \
          FOR UPDATE OF version, entry",
    )
    .bind(&job.org_id)
    .bind(entry_id)
    .bind(version_id)
    .fetch_optional(&mut *transaction)
    .await?;

    match state {
        None => {
            delete_claim(&mut transaction, job, lease_token).await?;
        }
        Some(state) if state.deleted_at.is_some() => {
            sqlx::query(
                "UPDATE briefcase.object_cleanup_jobs \
                    SET status = 'object_deleted', deletion_batch_id = $4, \
                        object_deleted_at = clock_timestamp(), lease_token = NULL, \
                        lease_expires_at = NULL, last_error_code = NULL \
                  WHERE org_id = $1 AND cleanup_id = $2 \
                    AND status = 'processing' AND lease_token = $3",
            )
            .bind(&job.org_id)
            .bind(job.cleanup_id)
            .bind(lease_token)
            .bind(state.deletion_batch_id)
            .execute(&mut *transaction)
            .await?;
        }
        Some(state) if state.current_version_id != Some(version_id) => {
            sqlx::query(
                "DELETE FROM briefcase.entry_versions \
                  WHERE org_id = $1 AND entry_id = $2 AND version_id = $3",
            )
            .bind(&job.org_id)
            .bind(entry_id)
            .bind(version_id)
            .execute(&mut *transaction)
            .await?;
            delete_claim(&mut transaction, job, lease_token).await?;
        }
        Some(_) => {
            sqlx::query(
                "UPDATE briefcase.object_cleanup_jobs \
                    SET status = 'object_deleted', object_deleted_at = clock_timestamp(), \
                        lease_token = NULL, lease_expires_at = NULL, \
                        last_error_code = 'cleanup_current_version_invariant' \
                  WHERE org_id = $1 AND cleanup_id = $2 \
                    AND status = 'processing' AND lease_token = $3",
            )
            .bind(&job.org_id)
            .bind(job.cleanup_id)
            .bind(lease_token)
            .execute(&mut *transaction)
            .await?;
            warn!(
                event = "object_cleanup_invariant_failed",
                cleanup_id = %job.cleanup_id,
                error_code = "cleanup_current_version_invariant",
                "provider object was deleted but current-version metadata was retained"
            );
        }
    }
    transaction.commit().await?;
    Ok(true)
}

async fn lock_claim(
    transaction: &mut Transaction<'_, Postgres>,
    job: &ClaimedCleanup,
    lease_token: Uuid,
) -> Result<bool, sqlx::Error> {
    let cleanup_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT cleanup_id FROM briefcase.object_cleanup_jobs \
          WHERE org_id = $1 AND cleanup_id = $2 \
            AND status = 'processing' AND lease_token = $3 \
          FOR UPDATE",
    )
    .bind(&job.org_id)
    .bind(job.cleanup_id)
    .bind(lease_token)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(cleanup_id.is_some())
}

async fn delete_claim(
    transaction: &mut Transaction<'_, Postgres>,
    job: &ClaimedCleanup,
    lease_token: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM briefcase.object_cleanup_jobs \
          WHERE org_id = $1 AND cleanup_id = $2 \
            AND status = 'processing' AND lease_token = $3",
    )
    .bind(&job.org_id)
    .bind(job.cleanup_id)
    .bind(lease_token)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn cancel_claim(
    pool: &PgPool,
    job: &ClaimedCleanup,
    lease_token: Uuid,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    delete_claim(&mut transaction, job, lease_token).await?;
    transaction.commit().await
}

async fn retry_claim(
    pool: &PgPool,
    job: &ClaimedCleanup,
    lease_token: Uuid,
    retry_base: Duration,
    max_retry_delay: Duration,
    error_code: &'static str,
) -> Result<ProcessOutcome, CleanupBatchError> {
    // Once exponential backoff reaches its configured cap, larger attempt
    // values cannot change the result and must not create an unbounded loop.
    let attempt = u16::try_from(job.attempt_count).unwrap_or(u16::MAX).min(64);
    let delay = retry_delay(retry_base, max_retry_delay, attempt, job.cleanup_id);
    let delay_millis = duration_millis_rounded_up(delay)?;
    let result = sqlx::query(
        "UPDATE briefcase.object_cleanup_jobs \
            SET status = 'pending', \
                available_at = clock_timestamp() \
                    + ($4::bigint * interval '1 millisecond'), \
                lease_token = NULL, lease_expires_at = NULL, last_error_code = $5 \
          WHERE org_id = $1 AND cleanup_id = $2 \
            AND status = 'processing' AND lease_token = $3",
    )
    .bind(&job.org_id)
    .bind(job.cleanup_id)
    .bind(lease_token)
    .bind(delay_millis)
    .bind(error_code)
    .execute(pool)
    .await?;
    if result.rows_affected() == 1 {
        warn!(
            event = "object_cleanup_retry_scheduled",
            cleanup_id = %job.cleanup_id,
            cleanup_kind = job.cleanup_kind.as_str(),
            attempt = job.attempt_count,
            error_code,
            "provider cleanup scheduled for retry"
        );
    } else {
        warn!(
            event = "object_cleanup_lease_lost",
            cleanup_id = %job.cleanup_id,
            cleanup_kind = job.cleanup_kind.as_str(),
            "cleanup failure ignored after lease loss"
        );
    }
    Ok(ProcessOutcome::Retried)
}

fn provider_error_code(error: &ObjectStoreError) -> &'static str {
    match error {
        ObjectStoreError::NotFound => "cleanup_storage_not_found",
        ObjectStoreError::Conflict => "cleanup_storage_conflict",
        ObjectStoreError::InvalidConfiguration => "cleanup_storage_configuration_invalid",
        ObjectStoreError::Unavailable => "cleanup_storage_unavailable",
        ObjectStoreError::Internal(_) => "cleanup_storage_internal",
    }
}

fn duration_millis_rounded_up(duration: Duration) -> Result<i64, CleanupBatchError> {
    let has_submillisecond_remainder = !duration.subsec_nanos().is_multiple_of(1_000_000);
    let rounded = duration
        .as_millis()
        .saturating_add(u128::from(has_submillisecond_remainder))
        .max(1);
    i64::try_from(rounded).map_err(|_| CleanupBatchError::DurationNotRepresentable)
}

#[derive(Debug, sqlx::FromRow)]
struct DeletionBatch {
    org_id: String,
    deletion_batch_id: Uuid,
}

async fn finalize_deletion_batches(pool: &PgPool, batch_size: i64) -> Result<u64, sqlx::Error> {
    let batches = sqlx::query_as::<_, DeletionBatch>(
        "SELECT org_id, deletion_batch_id \
           FROM briefcase.entries \
          WHERE deletion_batch_id IS NOT NULL \
            AND purge_after <= clock_timestamp() \
          GROUP BY org_id, deletion_batch_id \
          ORDER BY min(purge_after), org_id, deletion_batch_id \
          LIMIT $1",
    )
    .bind(batch_size)
    .fetch_all(pool)
    .await?;
    let mut purged = 0_u64;
    for batch in batches {
        if finalize_deletion_batch(pool, &batch).await? {
            purged += 1;
        }
    }
    Ok(purged)
}

async fn finalize_deletion_batch(
    pool: &PgPool,
    batch: &DeletionBatch,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let Some(entries) = lock_eligible_deletion_batch(&mut transaction, batch).await? else {
        transaction.commit().await?;
        return Ok(false);
    };
    if has_unconfirmed_version(&mut transaction, batch).await?
        || has_unresolved_multipart(&mut transaction, batch, &entries).await?
    {
        transaction.commit().await?;
        return Ok(false);
    }

    let deleted = delete_deletion_batch_metadata(&mut transaction, batch, &entries).await?;
    transaction.commit().await?;
    if deleted > 0 {
        info!(
            event = "deletion_batch_purged",
            deletion_batch_id = %batch.deletion_batch_id,
            entry_count = deleted,
            "expired deletion batch permanently purged"
        );
    }
    Ok(deleted > 0)
}

async fn lock_eligible_deletion_batch(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &DeletionBatch,
) -> Result<Option<Vec<Uuid>>, sqlx::Error> {
    let acquired = sqlx::query_scalar::<_, bool>(
        "SELECT pg_try_advisory_xact_lock( \
             hashtextextended($1 || ':purge:' || $2::text, 0) \
         )",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !acquired {
        return Ok(None);
    }

    let entries = sqlx::query_scalar::<_, Uuid>(
        "SELECT entry_id FROM briefcase.entries \
          WHERE org_id = $1 AND deletion_batch_id = $2 \
          ORDER BY entry_id \
          FOR UPDATE",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .fetch_all(&mut **transaction)
    .await?;
    if entries.is_empty() {
        return Ok(None);
    }
    let eligible = sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE( \
                    bool_and( \
                        deleted_at IS NOT NULL \
                        AND purge_after <= clock_timestamp() \
                    ), \
                    false \
                ) \
           FROM briefcase.entries \
          WHERE org_id = $1 AND deletion_batch_id = $2",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(eligible.then_some(entries))
}

async fn has_unconfirmed_version(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &DeletionBatch,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 \
               FROM briefcase.entry_versions AS version \
               JOIN briefcase.entries AS entry \
                 ON entry.org_id = version.org_id \
                AND entry.entry_id = version.entry_id \
              WHERE entry.org_id = $1 \
                AND entry.deletion_batch_id = $2 \
                AND NOT EXISTS ( \
                    SELECT 1 FROM briefcase.object_cleanup_jobs AS cleanup \
                     WHERE cleanup.org_id = version.org_id \
                       AND cleanup.cleanup_kind = 'version_delete' \
                       AND cleanup.source_entry_id = version.entry_id \
                       AND cleanup.source_version_id = version.version_id \
                       AND cleanup.status = 'object_deleted' \
                ) \
         )",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .fetch_one(&mut **transaction)
    .await
}

async fn has_unresolved_multipart(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &DeletionBatch,
    entries: &[Uuid],
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 FROM briefcase.multipart_uploads AS upload \
              WHERE upload.org_id = $1 \
                AND upload.status <> 'completed' \
                AND ( \
                    upload.parent_entry_id = ANY($2::uuid[]) \
                    OR upload.completed_entry_id = ANY($2::uuid[]) \
                ) \
         )",
    )
    .bind(&batch.org_id)
    .bind(entries)
    .fetch_one(&mut **transaction)
    .await
}

async fn delete_deletion_batch_metadata(
    transaction: &mut Transaction<'_, Postgres>,
    batch: &DeletionBatch,
    entries: &[Uuid],
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "DELETE FROM briefcase.multipart_uploads \
          WHERE org_id = $1 AND status = 'completed' \
            AND ( \
                parent_entry_id = ANY($2::uuid[]) \
                OR completed_entry_id = ANY($2::uuid[]) \
            )",
    )
    .bind(&batch.org_id)
    .bind(entries)
    .execute(&mut **transaction)
    .await?;
    let deleted = sqlx::query(
        "DELETE FROM briefcase.entries \
          WHERE org_id = $1 AND deletion_batch_id = $2",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    sqlx::query(
        "DELETE FROM briefcase.object_cleanup_jobs \
          WHERE org_id = $1 \
            AND (deletion_batch_id = $2 OR source_entry_id = ANY($3::uuid[]))",
    )
    .bind(&batch.org_id)
    .bind(batch.deletion_batch_id)
    .bind(entries)
    .execute(&mut **transaction)
    .await?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        ClaimedCleanup, CleanupKind, cleanup_target, duration_millis_rounded_up,
        provider_error_code,
    };
    use crate::{application::ports::ObjectStoreError, domain::storage::EncryptionMode};

    fn cleanup(backend: &str) -> ClaimedCleanup {
        ClaimedCleanup {
            org_id: "org-acme".to_owned(),
            cleanup_id: uuid::Uuid::from_u128(1),
            cleanup_kind: CleanupKind::VersionDelete.as_str().to_owned(),
            source_entry_id: Some(uuid::Uuid::from_u128(2)),
            source_version_id: Some(uuid::Uuid::from_u128(3)),
            source_upload_id: None,
            storage_backend: backend.to_owned(),
            storage_config_id: None,
            bucket_name: "objects.example".to_owned(),
            storage_region: "ap-south-1".to_owned(),
            storage_prefix: "tenant".to_owned(),
            storage_role_arn: None,
            storage_encryption_mode: "sse_s3".to_owned(),
            storage_kms_key_arn: None,
            object_key: "versions/object".to_owned(),
            object_version_id: Some("exact-version".to_owned()),
            provider_upload_id: None,
            attempt_count: 1,
        }
    }

    #[test]
    fn platform_cleanup_uses_the_persisted_descriptor() {
        let job = cleanup("platform");
        let Ok((target, key)) = cleanup_target(&job) else {
            panic!("platform target should be valid");
        };
        assert_eq!(target.bucket, "objects.example");
        assert_eq!(target.region, "ap-south-1");
        assert_eq!(target.prefix, "tenant");
        assert_eq!(target.encryption, EncryptionMode::SseS3);
        assert!(target.role_arn.is_none());
        assert!(target.external_id.is_none());
        assert_eq!(key.as_str(), "versions/object");
    }

    #[test]
    fn organization_cleanup_derives_external_id_server_side() {
        let mut job = cleanup("organization");
        job.storage_config_id = Some(uuid::Uuid::from_u128(4));
        job.storage_role_arn = Some("arn:aws:iam::123456789012:role/briefcase".to_owned());
        let Ok((target, _)) = cleanup_target(&job) else {
            panic!("organization target should be valid");
        };
        assert_eq!(
            target.role_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/briefcase")
        );
        assert!(target.external_id.is_some());
    }

    #[test]
    fn cleanup_telemetry_classification_never_exposes_provider_details() {
        let error = ObjectStoreError::Internal(anyhow::anyhow!("secret provider detail"));
        assert_eq!(provider_error_code(&error), "cleanup_storage_internal");
    }

    #[test]
    fn retry_milliseconds_round_up_and_remain_positive() {
        assert!(matches!(
            duration_millis_rounded_up(Duration::from_nanos(1)),
            Ok(1)
        ));
        assert!(matches!(
            duration_millis_rounded_up(Duration::from_millis(250)),
            Ok(250)
        ));
    }
}
