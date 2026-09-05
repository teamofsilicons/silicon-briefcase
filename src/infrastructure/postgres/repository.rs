//! Low-level transactional queries shared by higher-level repositories.

use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::application::context::TestingEnvironmentContext;

use super::{
    AuditEventRow, EntryRow, MultipartPartRow, OutboxEventRow, TenantContext,
    begin_tenant_transaction, models::entry_columns,
};

pub(crate) const STALE_TESTING_ENVIRONMENT_CONTEXT: &str =
    "testing environment changed while the request was in progress";

/// Inputs for an audit record written in the current tenant transaction.
#[derive(Clone, Debug)]
pub struct NewAuditEvent {
    /// `UUIDv7` audit event identifier.
    pub audit_id: Uuid,
    /// Entry targeted by the action, when applicable.
    pub entry_id: Option<Uuid>,
    /// Stable action name.
    pub action: String,
    /// Redacted structured metadata.
    pub metadata: Value,
}

/// Inputs for an event inserted into the transactional outbox.
#[derive(Clone, Debug)]
pub struct NewOutboxEvent {
    /// `UUIDv7` event identifier.
    pub event_id: Uuid,
    /// Delivery topic.
    pub topic: String,
    /// Aggregate type.
    pub aggregate_type: String,
    /// Aggregate identifier.
    pub aggregate_id: String,
    /// Optional monotonic aggregate version.
    pub aggregate_version: Option<i64>,
    /// Versioned, non-secret event payload.
    pub payload: Value,
    /// Earliest delivery time.
    pub available_at: OffsetDateTime,
}

/// Cloneable PostgreSQL repository entry point.
#[derive(Clone, Debug)]
pub struct PostgresRepository {
    pool: PgPool,
    test_pool: Option<PgPool>,
}

impl PostgresRepository {
    /// Wraps a configured runtime pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self {
            pool,
            test_pool: None,
        }
    }

    /// Adds the separately configured shared sandbox database.
    #[must_use]
    pub fn with_test_pool(mut self, test_pool: PgPool) -> Self {
        self.test_pool = Some(test_pool);
        self
    }

    /// Borrows the underlying pool for readiness and shutdown coordination.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Chooses the database plane named by an authenticated tenant context.
    pub(crate) fn pool_for(&self, context: &TenantContext) -> Result<&PgPool, sqlx::Error> {
        if context.testing_environment_id().is_some() {
            self.test_pool.as_ref().ok_or_else(|| {
                sqlx::Error::Configuration(
                    "the Briefcase testing database is not configured".into(),
                )
            })
        } else {
            Ok(&self.pool)
        }
    }

    /// Borrows the configured sandbox pool, when testing environments are enabled.
    #[must_use]
    pub const fn test_pool(&self) -> Option<&PgPool> {
        self.test_pool.as_ref()
    }

    /// Starts a tenant-isolated transaction for one authenticated request.
    ///
    /// # Errors
    ///
    /// Returns an error if a connection cannot be acquired or the tenant-local
    /// PostgreSQL settings cannot be installed.
    pub async fn begin<'pool>(
        &'pool self,
        context: &TenantContext,
    ) -> Result<Transaction<'pool, Postgres>, sqlx::Error> {
        let transaction = begin_tenant_transaction(self.pool_for(context)?, context).await?;
        if let (Some(environment_id), Some(control_version)) = (
            context.testing_environment_id(),
            context.testing_environment_control_version(),
        ) && !self
            .testing_environment_is_current(TestingEnvironmentContext::new(
                environment_id,
                control_version,
            ))
            .await?
        {
            transaction.rollback().await?;
            return Err(sqlx::Error::Protocol(
                STALE_TESTING_ENVIRONMENT_CONTEXT.to_owned(),
            ));
        }
        Ok(transaction)
    }

    /// Revalidates a data-plane generation through the production control DB.
    ///
    /// Call this only after the test transaction has acquired its shared clean
    /// fence. A clean that won the exclusive side first advances the version
    /// before releasing that fence, so a waiter cannot publish stale work.
    pub(crate) async fn testing_environment_is_current(
        &self,
        environment: TestingEnvironmentContext,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT briefcase.testing_environment_version_matches($1, $2)",
        )
        .bind(environment.id())
        .bind(environment.control_version())
        .fetch_one(&self.pool)
        .await
    }

    /// Loads an entry visible to the transaction's organization.
    ///
    /// This primitive does not apply application permission policy. Callers
    /// must authorize the row and write its required audit event before commit.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot execute or decode the query.
    pub async fn find_entry(
        transaction: &mut Transaction<'_, Postgres>,
        entry_id: Uuid,
    ) -> Result<Option<EntryRow>, sqlx::Error> {
        sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
        ))
        .bind(entry_id)
        .fetch_optional(&mut **transaction)
        .await
    }

    /// Locks and loads an entry before a metadata or lifecycle mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot execute or decode the query.
    pub async fn lock_entry(
        transaction: &mut Transaction<'_, Postgres>,
        entry_id: Uuid,
    ) -> Result<Option<EntryRow>, sqlx::Error> {
        sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
              FOR UPDATE",
        ))
        .bind(entry_id)
        .fetch_optional(&mut **transaction)
        .await
    }

    /// Inserts an audit event in the same transaction as its domain action.
    ///
    /// The schema serializes retention per entry and keeps only its latest 100
    /// events.
    ///
    /// # Errors
    ///
    /// Returns an error if the event violates tenant or projection constraints.
    pub async fn insert_audit_event(
        transaction: &mut Transaction<'_, Postgres>,
        context: &TenantContext,
        event: &NewAuditEvent,
    ) -> Result<AuditEventRow, sqlx::Error> {
        sqlx::query_as::<_, AuditEventRow>(
            "INSERT INTO briefcase.audit_events ( \
                    org_id, audit_id, entry_id, actor_type, actor_id, origin_app_id, \
                    action, request_id, metadata \
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             RETURNING org_id, audit_id, entry_id, actor_type, actor_id, origin_app_id, \
                       action, request_id, metadata, occurred_at",
        )
        .bind(context.org_id())
        .bind(event.audit_id)
        .bind(event.entry_id)
        .bind(context.actor_type())
        .bind(context.actor_id())
        .bind(context.origin_app_id())
        .bind(&event.action)
        .bind(context.request_id())
        .bind(&event.metadata)
        .fetch_one(&mut **transaction)
        .await
    }

    /// Inserts a delivery event in the current domain transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the event violates tenant or payload constraints.
    pub async fn insert_outbox_event(
        transaction: &mut Transaction<'_, Postgres>,
        context: &TenantContext,
        event: &NewOutboxEvent,
    ) -> Result<OutboxEventRow, sqlx::Error> {
        sqlx::query_as::<_, OutboxEventRow>(
            "INSERT INTO briefcase.outbox_events ( \
                    org_id, event_id, topic, aggregate_type, aggregate_id, \
                    aggregate_version, payload, available_at \
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING org_id, event_id, topic, aggregate_type, aggregate_id, \
                       aggregate_version, payload, status, attempt_count, available_at, \
                       lease_token, lease_expires_at, last_error, delivered_at, \
                       created_at, updated_at",
        )
        .bind(context.org_id())
        .bind(event.event_id)
        .bind(&event.topic)
        .bind(&event.aggregate_type)
        .bind(&event.aggregate_id)
        .bind(event.aggregate_version)
        .bind(&event.payload)
        .bind(event.available_at)
        .fetch_one(&mut **transaction)
        .await
    }

    /// Claims available outbox work with one bounded lease.
    ///
    /// Pending jobs and jobs whose prior processing lease expired are claimed
    /// with `FOR UPDATE SKIP LOCKED`, allowing independent workers to scale.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot claim or decode the rows.
    pub async fn claim_outbox(
        transaction: &mut Transaction<'_, Postgres>,
        batch_size: u32,
        lease_token: Uuid,
        lease_expires_at: OffsetDateTime,
    ) -> Result<Vec<OutboxEventRow>, sqlx::Error> {
        sqlx::query_as::<_, OutboxEventRow>(
            "WITH candidates AS ( \
                 SELECT event_id \
                   FROM briefcase.outbox_events \
                  WHERE org_id = briefcase.current_org_id() \
                    AND ( \
                        (status = 'pending' AND available_at <= clock_timestamp()) \
                        OR (status = 'processing' AND lease_expires_at <= clock_timestamp()) \
                    ) \
                  ORDER BY available_at, created_at, event_id \
                  FOR UPDATE SKIP LOCKED \
                  LIMIT $1 \
             ) \
             UPDATE briefcase.outbox_events AS event \
                SET status = 'processing', \
                    attempt_count = event.attempt_count + 1, \
                    lease_token = $2, \
                    lease_expires_at = $3 \
               FROM candidates \
              WHERE event.org_id = briefcase.current_org_id() \
                AND event.event_id = candidates.event_id \
             RETURNING event.org_id, event.event_id, event.topic, event.aggregate_type, \
                       event.aggregate_id, event.aggregate_version, event.payload, \
                       event.status, event.attempt_count, event.available_at, \
                       event.lease_token, event.lease_expires_at, event.last_error, \
                       event.delivered_at, event.created_at, event.updated_at",
        )
        .bind(i64::from(batch_size))
        .bind(lease_token)
        .bind(lease_expires_at)
        .fetch_all(&mut **transaction)
        .await
    }

    /// Marks a leased outbox event delivered if the lease still belongs to the caller.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot update the row.
    pub async fn mark_outbox_delivered(
        transaction: &mut Transaction<'_, Postgres>,
        event_id: Uuid,
        lease_token: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE briefcase.outbox_events \
                SET status = 'delivered', \
                    lease_token = NULL, \
                    lease_expires_at = NULL, \
                    last_error = NULL, \
                    delivered_at = clock_timestamp() \
              WHERE org_id = briefcase.current_org_id() \
                AND event_id = $1 \
                AND status = 'processing' \
                AND lease_token = $2",
        )
        .bind(event_id)
        .bind(lease_token)
        .execute(&mut **transaction)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Releases a leased outbox event for retry or terminal dead-letter state.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot update the row.
    pub async fn release_outbox_failure(
        transaction: &mut Transaction<'_, Postgres>,
        event_id: Uuid,
        lease_token: Uuid,
        retry_at: OffsetDateTime,
        redacted_error: &str,
        terminal: bool,
    ) -> Result<bool, sqlx::Error> {
        let next_status = if terminal { "dead_letter" } else { "pending" };
        let result = sqlx::query(
            "UPDATE briefcase.outbox_events \
                SET status = $3, \
                    available_at = $4, \
                    lease_token = NULL, \
                    lease_expires_at = NULL, \
                    last_error = $5 \
              WHERE org_id = briefcase.current_org_id() \
                AND event_id = $1 \
                AND status = 'processing' \
                AND lease_token = $2",
        )
        .bind(event_id)
        .bind(lease_token)
        .bind(next_status)
        .bind(retry_at)
        .bind(redacted_error)
        .execute(&mut **transaction)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Replaces one multipart part record after a provider upload succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if the part does not belong to the tenant upload or its
    /// metadata violates S3 bounds.
    pub async fn upsert_multipart_part(
        transaction: &mut Transaction<'_, Postgres>,
        upload_id: Uuid,
        part_number: u32,
        etag: &str,
        size_bytes: u64,
        checksum_sha256: &[u8],
    ) -> Result<MultipartPartRow, sqlx::Error> {
        let part_number =
            i32::try_from(part_number).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let size_bytes =
            i64::try_from(size_bytes).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        sqlx::query_as::<_, MultipartPartRow>(
            "INSERT INTO briefcase.multipart_parts ( \
                    org_id, upload_id, part_number, etag, size_bytes, checksum_sha256 \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5) \
             ON CONFLICT (org_id, upload_id, part_number) DO UPDATE \
                 SET etag = EXCLUDED.etag, \
                     size_bytes = EXCLUDED.size_bytes, \
                     checksum_sha256 = EXCLUDED.checksum_sha256, \
                     uploaded_at = clock_timestamp() \
             RETURNING org_id, upload_id, part_number, etag, size_bytes, \
                       checksum_sha256, uploaded_at",
        )
        .bind(upload_id)
        .bind(part_number)
        .bind(etag)
        .bind(size_bytes)
        .bind(checksum_sha256)
        .fetch_one(&mut **transaction)
        .await
    }

    /// Lists a multipart session's recorded parts in completion order.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot execute or decode the query.
    pub async fn list_multipart_parts(
        transaction: &mut Transaction<'_, Postgres>,
        upload_id: Uuid,
    ) -> Result<Vec<MultipartPartRow>, sqlx::Error> {
        sqlx::query_as::<_, MultipartPartRow>(
            "SELECT org_id, upload_id, part_number, etag, size_bytes, \
                    checksum_sha256, uploaded_at \
               FROM briefcase.multipart_parts \
              WHERE org_id = briefcase.current_org_id() AND upload_id = $1 \
              ORDER BY part_number",
        )
        .bind(upload_id)
        .fetch_all(&mut **transaction)
        .await
    }
}
