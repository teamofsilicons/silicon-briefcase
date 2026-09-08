//! Durable cleanup of unpublished delegated objects, never published versions.

use std::time::Duration;

use futures::{StreamExt as _, stream};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    application::ports::{ObjectKey, ObjectMetadata, ObjectStore, ObjectStoreError, StorageTarget},
    domain::storage::EncryptionMode,
};

use super::policy::retry_delay;

#[derive(Default)]
pub(super) struct Stats {
    pub(super) expired: u64,
    pub(super) completed: u64,
    pub(super) deferred: u64,
}

#[derive(sqlx::FromRow)]
struct Job {
    org_id: String,
    upload_id: Uuid,
    size_bytes: i64,
    bucket_name: String,
    storage_region: String,
    storage_prefix: String,
    storage_role_arn: Option<String>,
    storage_external_id: Option<String>,
    storage_encryption_mode: String,
    storage_kms_key_arn: Option<String>,
    object_key: String,
    object_version_id: Option<String>,
    object_etag: Option<String>,
    object_checksum_value: Option<String>,
    provider_upload_id: Option<String>,
    provider_completion_started: bool,
    cleanup_attempts: i32,
}

/// Expires abandoned writers, then processes a bounded cleanup batch per plane.
pub(super) async fn process_batch<O: ObjectStore + ?Sized>(
    pool: &PgPool,
    objects: &O,
    batch_size: i64,
    concurrency: usize,
    lease_millis: i64,
    retry_base: Duration,
    retry_max: Duration,
) -> Result<Stats, sqlx::Error> {
    let expired = sqlx::query(
        "WITH candidates AS ( \
            SELECT org_id, upload_id FROM briefcase.delegated_uploads \
             WHERE (status IN ('reserved','receiving','staged') AND expires_at <= clock_timestamp()) \
                OR (status = 'receiving' AND lease_expires_at <= clock_timestamp()) \
             ORDER BY expires_at, org_id, upload_id LIMIT $1 FOR UPDATE SKIP LOCKED \
         ) UPDATE briefcase.delegated_uploads AS upload \
             SET status = CASE WHEN provider_write_started THEN 'cleanup_pending' ELSE 'expired' END, \
                 cleanup_after = CASE WHEN provider_write_started \
                     THEN GREATEST(clock_timestamp(), \
                         COALESCE(lease_expires_at + interval '120 seconds', clock_timestamp()), \
                         COALESCE(provider_deadline_at + interval '120 seconds', clock_timestamp())) \
                     ELSE NULL END, \
                 capability_hash = NULL, lease_token = NULL, lease_expires_at = NULL, \
                 cleanup_last_error = 'reservation_expired' \
            FROM candidates WHERE upload.org_id = candidates.org_id AND upload.upload_id = candidates.upload_id",
    ).bind(batch_size).execute(pool).await?.rows_affected();
    let mut stats = Stats {
        expired,
        ..Stats::default()
    };
    let mut tasks = stream::iter(0..batch_size)
        .map(|_| async {
            let token = Uuid::now_v7();
            let Some(job) = claim(pool, token, lease_millis).await? else {
                return Ok::<_, sqlx::Error>(None);
            };
            let result = clean(pool, objects, &job, token).await?;
            if let Err(code) = result {
                let attempts = u16::try_from(job.cleanup_attempts).unwrap_or(u16::MAX);
                let delay = retry_delay(retry_base, retry_max, attempts, job.upload_id);
                let delay_millis = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
                sqlx::query(
                    "UPDATE briefcase.delegated_uploads \
                    SET cleanup_lease_token = NULL, cleanup_lease_expires_at = NULL, \
                        cleanup_after = clock_timestamp() + $4 * interval '1 millisecond', \
                        cleanup_last_error = $5 \
                  WHERE org_id = $1 AND upload_id = $2 AND status = 'cleanup_pending' \
                    AND cleanup_lease_token = $3",
                )
                .bind(&job.org_id)
                .bind(job.upload_id)
                .bind(token)
                .bind(delay_millis)
                .bind(code)
                .execute(pool)
                .await?;
            }
            Ok(Some(result.is_ok()))
        })
        .buffer_unordered(concurrency);
    while let Some(outcome) = tasks.next().await {
        match outcome? {
            Some(true) => stats.completed += 1,
            Some(false) => stats.deferred += 1,
            None => {}
        }
    }
    Ok(stats)
}

async fn claim(pool: &PgPool, token: Uuid, lease_millis: i64) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as::<_, Job>(
        "WITH candidate AS ( \
            SELECT org_id, upload_id FROM briefcase.delegated_uploads \
             WHERE status = 'cleanup_pending' AND cleanup_after <= clock_timestamp() \
               AND (cleanup_lease_token IS NULL OR cleanup_lease_expires_at <= clock_timestamp()) \
             ORDER BY cleanup_after, org_id, upload_id LIMIT 1 FOR UPDATE SKIP LOCKED \
         ) UPDATE briefcase.delegated_uploads AS upload \
            SET cleanup_lease_token = $1, \
                cleanup_lease_expires_at = clock_timestamp() + $2 * interval '1 millisecond', \
                cleanup_attempts = LEAST(cleanup_attempts::bigint + 1, 2147483647)::integer \
           FROM candidate WHERE upload.org_id = candidate.org_id AND upload.upload_id = candidate.upload_id \
         RETURNING upload.org_id, upload.upload_id, upload.size_bytes, upload.bucket_name, \
            upload.storage_region, upload.storage_prefix, upload.storage_role_arn, \
            upload.storage_external_id, upload.storage_encryption_mode, upload.storage_kms_key_arn, \
            upload.object_key, upload.object_version_id, upload.object_etag, upload.object_checksum_value, \
            upload.provider_upload_id, upload.provider_completion_started, upload.cleanup_attempts",
    ).bind(token).bind(lease_millis).fetch_optional(pool).await
}

type CleanupResult = Result<(), &'static str>;

/// CAS checks precede external calls; `cleanup_pending` never transitions back to
/// publishable state. A stale worker may at most repeat an exact idempotent
/// cleanup, and cannot discard another worker's descriptor or finish a commit.
async fn clean<O: ObjectStore + ?Sized>(
    pool: &PgPool,
    objects: &O,
    job: &Job,
    token: Uuid,
) -> Result<CleanupResult, sqlx::Error> {
    let Ok((target, key)) = target(job) else {
        return Ok(Err("invalid_provider_descriptor"));
    };
    if !owns_claim(pool, job, token).await? {
        return Ok(Err("cleanup_lease_lost"));
    }
    let mut multipart_known = job.provider_upload_id.is_some();
    if job.size_bytes > 100 * 1024 * 1024 || multipart_known {
        let Ok(mut sessions) = objects.list_multipart_uploads_for_key(&target, &key).await else {
            return Ok(Err("multipart_discovery_unavailable"));
        };
        if !multipart_known && let Some(id) = sessions.first() {
            let recorded = sqlx::query(
                "UPDATE briefcase.delegated_uploads SET provider_upload_id = $4 \
                  WHERE org_id = $1 AND upload_id = $2 AND status = 'cleanup_pending' \
                    AND cleanup_lease_token = $3 AND cleanup_lease_expires_at > clock_timestamp() \
                    AND provider_upload_id IS NULL",
            )
            .bind(&job.org_id)
            .bind(job.upload_id)
            .bind(token)
            .bind(id)
            .execute(pool)
            .await?
            .rows_affected();
            if recorded != 1 {
                return Ok(Err("cleanup_lease_lost"));
            }
            multipart_known = true;
        }
        if let Some(id) = &job.provider_upload_id
            && !sessions.contains(id)
        {
            sessions.push(id.clone());
        }
        for id in sessions {
            if !owns_claim(pool, job, token).await? {
                return Ok(Err("cleanup_lease_lost"));
            }
            if objects.abort_multipart(&target, &key, &id).await.is_err() {
                return Ok(Err("multipart_abort_unavailable"));
            }
            if !matches!(
                objects.multipart_upload_is_empty(&target, &key, &id).await,
                Ok(true)
            ) {
                return Ok(Err("multipart_parts_unconfirmed"));
            }
        }
    }
    if !owns_claim(pool, job, token).await? {
        return Ok(Err("cleanup_lease_lost"));
    }
    let evidence_known = job.object_version_id.is_some()
        || job.object_etag.is_some()
        || job.object_checksum_value.is_some();
    match objects
        .head(&target, &key, job.object_version_id.as_deref())
        .await
    {
        Ok(metadata) => {
            // Persist the provider's exact version BEFORE deletion. A lost DB
            // response retains the ledger and a future worker can repeat it.
            if !record_version(pool, job, token, &metadata).await? {
                return Ok(Err("cleanup_lease_lost"));
            }
            let version = metadata
                .provider_version_id
                .as_deref()
                .or(job.object_version_id.as_deref());
            if objects.delete(&target, &key, version).await.is_err() {
                return Ok(Err("object_delete_unavailable"));
            }
            match objects.head(&target, &key, version).await {
                Err(ObjectStoreError::NotFound) => {}
                Ok(_) => return Ok(Err("object_deletion_unconfirmed")),
                Err(_) => return Ok(Err("object_head_unavailable")),
            }
        }
        Err(ObjectStoreError::NotFound)
            if evidence_known || (!job.provider_completion_started && multipart_known) => {}
        // A provider PUT/completion with a lost acknowledgement may still
        // produce an object. Absence alone cannot prove that attempt failed.
        // Keep the descriptor AND staging allocation for later reconciliation.
        Err(ObjectStoreError::NotFound) => return Ok(Err("provider_outcome_unknown")),
        Err(_) => return Ok(Err("object_head_unavailable")),
    }
    let completed = sqlx::query(
        "UPDATE briefcase.delegated_uploads \
            SET status = CASE WHEN expires_at <= clock_timestamp() THEN 'expired' ELSE 'cancelled' END, \
                provider_write_started = false, provider_completion_started = false, \
                capability_hash = NULL, lease_token = NULL, lease_expires_at = NULL, \
                cleanup_lease_token = NULL, cleanup_lease_expires_at = NULL, \
                cleanup_after = NULL, cleanup_last_error = NULL \
          WHERE org_id = $1 AND upload_id = $2 AND status = 'cleanup_pending' \
            AND cleanup_lease_token = $3 AND cleanup_lease_expires_at > clock_timestamp()",
    ).bind(&job.org_id).bind(job.upload_id).bind(token).execute(pool).await?.rows_affected();
    Ok(if completed == 1 {
        Ok(())
    } else {
        Err("cleanup_lease_lost")
    })
}

async fn owns_claim(pool: &PgPool, job: &Job, token: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM briefcase.delegated_uploads \
          WHERE org_id = $1 AND upload_id = $2 AND status = 'cleanup_pending' \
            AND cleanup_lease_token = $3 AND cleanup_lease_expires_at > clock_timestamp())",
    )
    .bind(&job.org_id)
    .bind(job.upload_id)
    .bind(token)
    .fetch_one(pool)
    .await
}

async fn record_version(
    pool: &PgPool,
    job: &Job,
    token: Uuid,
    metadata: &ObjectMetadata,
) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        "UPDATE briefcase.delegated_uploads \
            SET object_version_id = COALESCE(object_version_id, $4), object_etag = COALESCE(object_etag, $5) \
          WHERE org_id = $1 AND upload_id = $2 AND status = 'cleanup_pending' \
            AND cleanup_lease_token = $3 AND cleanup_lease_expires_at > clock_timestamp() \
            AND (object_version_id IS NULL OR object_version_id IS NOT DISTINCT FROM $4)",
    ).bind(&job.org_id).bind(job.upload_id).bind(token)
        .bind(metadata.provider_version_id.as_deref()).bind(metadata.etag.as_deref())
        .execute(pool).await?.rows_affected();
    Ok(updated == 1)
}

fn target(job: &Job) -> Result<(StorageTarget, ObjectKey), ()> {
    let encryption = match job.storage_encryption_mode.as_str() {
        "sse_s3" => EncryptionMode::SseS3,
        "sse_kms" => EncryptionMode::SseKms,
        _ => return Err(()),
    };
    let key = ObjectKey::new(job.object_key.clone()).map_err(|_| ())?;
    Ok((
        StorageTarget {
            bucket: job.bucket_name.clone(),
            region: job.storage_region.clone(),
            prefix: job.storage_prefix.clone(),
            role_arn: job.storage_role_arn.clone(),
            external_id: job.storage_external_id.clone(),
            encryption,
            kms_key_arn: job.storage_kms_key_arn.clone(),
        },
        key,
    ))
}
