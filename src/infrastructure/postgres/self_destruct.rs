//! Keeping a self-destructing file (UNDERSTANDING.md §Self Destruct).

use serde_json::json;

use super::{
    PostgresRepository,
    metadata::common::{self, actor_kind, begin, load_entry, record_change},
    sharing::db,
};
use crate::{
    application::{context::ExecutionContext, service::MutationMetadata},
    domain::{ids::EntryId, permission::Capability},
    error::AppError,
};

fn repo_error(value: crate::application::service::MetadataRepositoryError) -> AppError {
    crate::api::mapping::metadata_error(value.into())
}

impl PostgresRepository {
    /// Stops a self-destruct timer so the file is kept.
    ///
    /// Only the file's creator, an organization administrator, or an owner may
    /// keep it. Anyone else who can see the file is refused; anyone who cannot
    /// see it is told it does not exist.
    pub(crate) async fn make_permanent(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        metadata: &MutationMetadata,
    ) -> Result<(), AppError> {
        const OPERATION: &str = "make_permanent";
        let mut request = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut request.transaction, context, id, false, true)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        if !entry
            .authorization(context.authorization())
            .allows(Capability::Read)
        {
            return Err(AppError::NotFound);
        }
        let actor = context.authorization().actor();
        let creator: (String, String) = sqlx::query_as(
            "SELECT created_by_type, created_by_id FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1",
        )
        .bind(id.as_uuid())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(db)?;
        let is_creator = creator.0 == actor_kind(actor.kind()) && creator.1 == actor.id().as_str();
        if !(is_creator || context.authorization().role().has_administrative_access()) {
            return Err(AppError::Forbidden);
        }
        if let common::IdempotencyClaim::Replay(_) = common::claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(id.as_uuid()),
        )
        .await
        .map_err(repo_error)?
        {
            return request.transaction.commit().await.map_err(db);
        }
        // The timer must still be running: once it passes, the file is gone
        // even if the sweep has not removed it yet.
        let cleared: Option<time::OffsetDateTime> = sqlx::query_scalar(
            "UPDATE briefcase.entries AS entry SET self_destruct_at = NULL \
               FROM (SELECT self_destruct_at FROM briefcase.entries \
                      WHERE org_id = briefcase.current_org_id() AND entry_id = $1 FOR UPDATE) AS previous \
              WHERE entry.org_id = briefcase.current_org_id() AND entry.entry_id = $1 \
                AND entry.deleted_at IS NULL AND previous.self_destruct_at > clock_timestamp() \
             RETURNING previous.self_destruct_at",
        )
        .bind(id.as_uuid())
        .fetch_optional(&mut *request.transaction)
        .await
        .map_err(db)?;
        let Some(previous) = cleared else {
            return Err(AppError::conflict("not_self_destructing"));
        };
        record_change(
            &mut request.transaction,
            &request.context,
            Some(id.as_uuid()),
            "entry.made_permanent.v1",
            "entry",
            &id.to_string(),
            json!({"previous_self_destruct_at": common::rfc3339(previous)}),
        )
        .await
        .map_err(repo_error)?;
        common::complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(id.as_uuid()),
        )
        .await
        .map_err(repo_error)?;
        request.transaction.commit().await.map_err(db)
    }
}
