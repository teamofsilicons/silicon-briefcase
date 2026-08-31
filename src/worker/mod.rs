//! Durable cross-tenant maintenance and outbox worker.

mod cleanup;
mod maintenance;
mod outbox;
mod policy;

use std::io;

use anyhow::Context as _;
use sqlx::PgPool;
use tokio::{
    signal,
    time::{MissedTickBehavior, interval},
};
use tracing::{error, info};

use crate::{
    application::ports::ObjectStore,
    config::{WorkerProcessSettings, WorkerSettings},
    infrastructure::{postgres, s3::S3ObjectStore},
};

/// Runs the durable worker until an interrupt or termination signal arrives.
///
/// The worker refuses to touch tenant tables unless its effective PostgreSQL
/// role is a superuser or has `BYPASSRLS`. API principals must never satisfy
/// that requirement.
///
/// # Errors
///
/// Returns an error when the database cannot be reached, the effective role
/// cannot safely perform cross-tenant work, worker durations cannot be encoded,
/// or shutdown signal registration fails.
pub async fn run(settings: WorkerProcessSettings) -> anyhow::Result<()> {
    let pool = postgres::connect(&settings.database, "silicon-briefcase-worker")
        .await
        .context("worker database connection failed")?;
    postgres::verify_cross_tenant_role(&pool).await?;

    let objects = S3ObjectStore::from_settings(&settings.s3).await;
    let runtime = WorkerRuntime::new(settings.worker, settings.s3.operation_timeout)?;
    let result = runtime.run(&pool, &objects).await;
    pool.close().await;
    result
}

struct WorkerRuntime {
    settings: WorkerSettings,
    batch_size: i64,
    lease_duration_millis: i64,
    cleanup_lease_duration_millis: i64,
}

impl WorkerRuntime {
    fn new(
        settings: WorkerSettings,
        object_operation_timeout: std::time::Duration,
    ) -> anyhow::Result<Self> {
        let batch_size = i64::try_from(settings.batch_size.get())
            .context("worker batch size cannot be represented by PostgreSQL")?;
        let lease_duration_millis = duration_millis(settings.lease_duration)
            .context("worker lease duration cannot be represented by PostgreSQL")?;
        let cleanup_lease_duration_millis = duration_millis(
            settings
                .lease_duration
                .saturating_add(object_operation_timeout),
        )
        .context("cleanup lease duration cannot be represented by PostgreSQL")?;
        duration_millis(settings.max_retry_delay)
            .context("worker retry delay cannot be represented by PostgreSQL")?;

        Ok(Self {
            settings,
            batch_size,
            lease_duration_millis,
            cleanup_lease_duration_millis,
        })
    }

    async fn run<O>(self, pool: &PgPool, objects: &O) -> anyhow::Result<()>
    where
        O: ObjectStore + ?Sized,
    {
        let mut poll_timer = interval(self.settings.poll_interval);
        let mut maintenance = interval(self.settings.maintenance_interval);
        poll_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        maintenance.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let shutdown = shutdown_signal();
        tokio::pin!(shutdown);

        info!(event = "worker_started", "background worker started");
        loop {
            tokio::select! {
                signal_result = &mut shutdown => {
                    signal_result.context("worker shutdown signal failed")?;
                    info!(event = "worker_shutdown", "background worker shutting down");
                    break;
                }
                _ = poll_timer.tick() => {
                    if let Err(error) = outbox::process_batch(
                        pool,
                        &self.settings,
                        self.batch_size,
                        self.lease_duration_millis,
                    ).await {
                        error!(
                            event = "outbox_batch_failed",
                            error_code = error.code(),
                            "outbox batch failed"
                        );
                    }
                }
                _ = maintenance.tick() => {
                    self.run_maintenance(pool, objects).await;
                }
            }
        }
        Ok(())
    }

    async fn run_maintenance<O>(&self, pool: &PgPool, objects: &O)
    where
        O: ObjectStore + ?Sized,
    {
        if let Ok(stats) = maintenance::run(pool).await {
            info!(
                event = "worker_maintenance_completed",
                indexed_entries = stats.indexed_entries,
                removed_search_documents = stats.removed_search_documents,
                expired_idempotency_records = stats.expired_idempotency_records,
                "worker database maintenance completed"
            );
        } else {
            error!(
                event = "worker_maintenance_failed",
                error_code = "maintenance_database_error",
                "worker database maintenance failed"
            );
        }

        match cleanup::process_batch(
            pool,
            objects,
            self.batch_size,
            self.settings.cleanup_concurrency.get(),
            self.cleanup_lease_duration_millis,
            self.settings.poll_interval,
            self.settings.max_retry_delay,
        )
        .await
        {
            Ok(stats) => info!(
                event = "object_cleanup_batch_completed",
                multipart_jobs_scheduled = stats.multipart_jobs_scheduled,
                version_jobs_scheduled = stats.version_jobs_scheduled,
                provider_operations_completed = stats.provider_operations_completed,
                provider_operations_retried = stats.provider_operations_retried,
                deletion_batches_purged = stats.deletion_batches_purged,
                "object cleanup batch completed"
            ),
            Err(error) => error!(
                event = "object_cleanup_batch_failed",
                error_code = error.code(),
                "object cleanup batch failed"
            ),
        }
    }
}

fn duration_millis(duration: std::time::Duration) -> Result<i64, std::num::TryFromIntError> {
    i64::try_from(duration.as_millis())
}

#[cfg(unix)]
async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> io::Result<()> {
    signal::ctrl_c().await
}
