//! Safe database-local projection and lease maintenance.

use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MaintenanceStats {
    pub(super) indexed_entries: u64,
    pub(super) removed_search_documents: u64,
    pub(super) expired_idempotency_records: u64,
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

    transaction.commit().await?;
    Ok(MaintenanceStats {
        indexed_entries,
        removed_search_documents,
        expired_idempotency_records,
    })
}
