//! Safe database-local projection and lease maintenance.

use sqlx::PgPool;

/// Notifications retained per recipient inbox.
///
/// The inbox is a live signal, not an archive: the audit history is the record
/// of what happened. Keeping the newest few hundred bounds inbox growth in an
/// organization that shares aggressively.
const RETAINED_NOTIFICATIONS_PER_RECIPIENT: i64 = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MaintenanceStats {
    pub(super) indexed_entries: u64,
    pub(super) removed_search_documents: u64,
    pub(super) expired_idempotency_records: u64,
    pub(super) pruned_notifications: u64,
}

pub(super) async fn run(pool: &PgPool) -> Result<MaintenanceStats, sqlx::Error> {
    let mut transaction = pool.begin().await?;

    let indexed_entries = sqlx::query(
        "INSERT INTO briefcase.search_documents (org_id, entry_id, filename) \
         SELECT entry.org_id, entry.entry_id, entry.name \
           FROM briefcase.entries AS entry \
          WHERE entry.entry_type = 'file' \
            AND entry.deleted_at IS NULL \
         ON CONFLICT (org_id, entry_id) DO UPDATE \
             SET filename = EXCLUDED.filename \
           WHERE briefcase.search_documents.filename IS DISTINCT FROM EXCLUDED.filename",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    let removed_search_documents = sqlx::query(
        "DELETE FROM briefcase.search_documents AS document \
          WHERE NOT EXISTS ( \
              SELECT 1 \
                FROM briefcase.entries AS entry \
               WHERE entry.org_id = document.org_id \
                 AND entry.entry_id = document.entry_id \
                 AND entry.entry_type = 'file' \
                 AND entry.deleted_at IS NULL \
          )",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    let expired_idempotency_records = sqlx::query(
        "DELETE FROM briefcase.idempotency_records \
          WHERE status = 'in_progress' \
            AND operation <> 'complete_multipart_upload' \
            AND expires_at <= clock_timestamp()",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    let expired_testing_environment_idempotency = sqlx::query(
        "DELETE FROM briefcase.testing_environment_idempotency \
          WHERE expires_at <= clock_timestamp()",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    let expired_idempotency_records =
        expired_idempotency_records + expired_testing_environment_idempotency;

    let pruned_notifications = sqlx::query(
        "DELETE FROM briefcase.notifications AS notification \
          USING ( \
              SELECT org_id, notification_id, \
                     row_number() OVER ( \
                         PARTITION BY org_id, recipient_type, recipient_id \
                         ORDER BY created_at DESC, notification_id DESC \
                     ) AS position \
                FROM briefcase.notifications \
          ) AS ranked \
          WHERE ranked.position > $1 \
            AND notification.org_id = ranked.org_id \
            AND notification.notification_id = ranked.notification_id",
    )
    .bind(RETAINED_NOTIFICATIONS_PER_RECIPIENT)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    sqlx::query("DELETE FROM briefcase.audit_events WHERE occurred_at < clock_timestamp() - interval '365 days'")
        .execute(&mut *transaction).await?;
    sqlx::query("UPDATE briefcase.api_contracts SET status='sunset',sunset_at=clock_timestamp() WHERE status='deprecated' AND GREATEST(deprecated_at,last_request_at) <= clock_timestamp()-interval '7 days'")
        .execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(MaintenanceStats {
        indexed_entries,
        removed_search_documents,
        expired_idempotency_records,
        pruned_notifications,
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn retirement_requires_seven_days_without_consumers() -> anyhow::Result<()> {
        let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
            return Ok(());
        };
        let pool = sqlx::PgPool::connect_with(
            url.parse::<sqlx::postgres::PgConnectOptions>()?
                .options([("search_path", "public")]),
        )
        .await?;
        crate::infrastructure::postgres::migrate(&pool).await?;
        let version = format!("test-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO briefcase.api_contracts(version,status,deprecated_at) VALUES($1,'deprecated',clock_timestamp()-interval '8 days')").bind(&version).execute(&pool).await?;
        let observed: String = sqlx::query_scalar("SELECT briefcase.observe_api_contract($1)")
            .bind(&version)
            .fetch_one(&pool)
            .await?;
        assert_eq!(observed, "deprecated");
        super::run(&pool).await?;
        let state: String =
            sqlx::query_scalar("SELECT status FROM briefcase.api_contracts WHERE version=$1")
                .bind(&version)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            state, "deprecated",
            "a recent consumer restarts the inactivity window"
        );
        sqlx::query("UPDATE briefcase.api_contracts SET last_request_at=clock_timestamp()-interval '8 days' WHERE version=$1").bind(&version).execute(&pool).await?;
        super::run(&pool).await?;
        let retired: String = sqlx::query_scalar("SELECT briefcase.observe_api_contract($1)")
            .bind(&version)
            .fetch_one(&pool)
            .await?;
        assert_eq!(retired, "sunset");
        sqlx::query("DELETE FROM briefcase.api_contracts WHERE version=$1")
            .bind(version)
            .execute(&pool)
            .await?;
        pool.close().await;
        Ok(())
    }
}
