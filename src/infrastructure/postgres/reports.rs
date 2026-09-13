//! Durable, tenant-isolated user-submitted bug reports.
use super::{
    PostgresRepository,
    metadata::common::{
        IdempotencyClaim, Result, begin, claim_idempotency, complete_idempotency, internal, map_sql,
    },
};
use crate::application::{context::ExecutionContext, service::MutationMetadata};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A report receipt is stable when the caller retries the same operation.
#[derive(Clone, Debug, Deserialize, Serialize, sqlx::FromRow)]
pub struct ReportReceipt {
    /// Durable report identifier.
    pub id: Uuid,
    /// Whether the report was durably accepted.
    pub accepted: bool,
}

impl PostgresRepository {
    /// Stores a report in the caller's actual production or testing plane.
    ///
    /// # Errors
    /// Returns an error if authorization projections, the lifecycle fence,
    /// idempotency identity, or the database transaction cannot be validated.
    pub async fn submit_report(
        &self,
        execution: &ExecutionContext,
        message: &str,
        pr: Option<&str>,
        isi: Option<&str>,
        metadata: &MutationMetadata,
    ) -> Result<ReportReceipt> {
        let mut request = begin(self, execution).await?;
        let claim = claim_idempotency(
            &mut request.transaction,
            &request.context,
            "submit_report",
            metadata,
            Some(Uuid::now_v7()),
        )
        .await?;
        let (id, replay) = match claim {
            IdempotencyClaim::Acquired(Some(id)) => (id, false),
            IdempotencyClaim::Replay(Some(id)) => (id, true),
            _ => return Err(internal("report idempotency record omitted identifier")),
        };
        if !replay {
            sqlx::query("INSERT INTO briefcase.bug_reports (org_id, report_id, actor_type, actor_id, message, pr_url, isi) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6)")
                .bind(id).bind(request.context.actor_type()).bind(request.context.actor_id())
                .bind(message).bind(pr).bind(isi)
                .execute(&mut *request.transaction).await.map_err(map_sql)?;
            complete_idempotency(
                &mut request.transaction,
                &request.context,
                "submit_report",
                metadata,
                Some(id),
            )
            .await?;
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(ReportReceipt { id, accepted: true })
    }
}
