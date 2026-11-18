//! Deferred delivery. An offer carries the original sender, rights and absolute expiry.
use super::{
    actor_kind,
    common::{RequestTransaction, Result},
    encode_access, map_sql,
};
use crate::{
    application::{context::ExecutionContext, service::GrantPermissionCommand},
    infrastructure::postgres::PermissionGrantRow,
};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

// Serialize sender approval with offers from the same sender, across drives.
pub(in crate::infrastructure::postgres) async fn lock_pair(
    transaction: &mut Transaction<'_, Postgres>,
    recipient: &str,
    sender: &str,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1),hashtext($2))")
        .bind(recipient)
        .bind(sender)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql)?;
    Ok(())
}

pub(super) async fn lock_recipients(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
) -> Result<()> {
    let mut ids: Vec<_> = context
        .review_recipients()
        .iter()
        .map(|r| r.id().as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        lock_pair(
            transaction,
            id,
            context.authorization().actor().id().as_str(),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn stage(
    request: &mut RequestTransaction<'_>,
    context: &ExecutionContext,
    command: &GrantPermissionCommand,
    id: Uuid,
) -> Result<Option<PermissionGrantRow>> {
    if !context.review_recipients().contains(&command.principal) {
        return Ok(None);
    }
    let sender = context.authorization().actor();
    sqlx::query_as(
        "INSERT INTO briefcase.share_reviews (grant_id,org_id,entry_id,entry_name,entry_type,
             principal_type,principal_id,granted_by_type,granted_by_id,access_mask,inherits_to_descendants,expires_at)
         SELECT $1,e.org_id,e.entry_id,e.name,e.entry_type,$3,$4,$5,$6,$7,$8,
                clock_timestamp()+make_interval(mins => $9)
         FROM briefcase.entries e JOIN briefcase.accounts a ON a.account_uuid=$4
         WHERE e.org_id=briefcase.current_org_id() AND e.entry_id=$2 AND NOT a.accept_shares_from_anyone
           AND NOT EXISTS (SELECT FROM briefcase.share_review_senders WHERE recipient=$4 AND sender=$6)
         ON CONFLICT (org_id,entry_id,principal_id,granted_by_id) WHERE expires_at IS NULL
         DO UPDATE SET access_mask=EXCLUDED.access_mask,inherits_to_descendants=EXCLUDED.inherits_to_descendants,
                       entry_name=EXCLUDED.entry_name
         RETURNING *, NULL::timestamptz AS revoked_at,NULL::text AS revoked_by_id,NULL::text AS revoked_by_type",
    ).bind(id).bind(command.entry_id.as_uuid()).bind(actor_kind(command.principal.kind()))
        .bind(command.principal.id().as_str()).bind(actor_kind(sender.kind())).bind(sender.id().as_str())
        .bind(encode_access(command.access)).bind(command.inherits_to_descendants)
        .bind(command.lifetime.map(crate::domain::lifetime::LifetimeMinutes::as_i32))
        .fetch_optional(&mut *request.transaction).await.map_err(map_sql)
}

pub(super) async fn find(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Option<PermissionGrantRow>> {
    sqlx::query_as(
        "SELECT r.*,NULL::timestamptz AS revoked_at,NULL::text AS revoked_by_type,NULL::text AS revoked_by_id
         FROM briefcase.share_reviews r WHERE org_id=briefcase.current_org_id() AND grant_id=$1",
    ).bind(id).fetch_optional(&mut **transaction).await.map_err(map_sql)
}
