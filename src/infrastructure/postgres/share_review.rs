//! Recipient-side delivery of offers. All decisions use the authenticated
//! reviewer's identity. Only the source drive changes while delivering an offer.
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    PostgresRepository, TenantContext,
    metadata::{
        common::{self, record_change},
        review::lock_pair,
    },
    sharing::db,
};
use crate::{application::context::ExecutionContext, error::AppError};

fn repo_error(error: crate::application::service::MetadataRepositoryError) -> AppError {
    crate::api::mapping::metadata_error(error.into())
}

impl PostgresRepository {
    pub(crate) async fn share_reviews(
        &self,
        ctx: &ExecutionContext,
        recipient: &str,
        cursor: Option<Uuid>,
    ) -> Result<Value, AppError> {
        let mut tx = self
            .begin(&TenantContext::from_execution(ctx))
            .await
            .map_err(db)?;
        let mut items: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('id',r.grant_id,'name',r.entry_name,'type',r.entry_type,
                'sender',jsonb_build_object('uuid',a.account_uuid,'id',a.public_id,'kind',a.kind,'display_name',a.display_name),
                'access',array_remove(ARRAY['read',CASE WHEN r.access_mask & 2 <> 0 THEN 'write' END,
                CASE WHEN r.access_mask & 4 <> 0 THEN 'update' END],NULL), 'inherit',r.inherits_to_descendants,
                'expires_at',r.expires_at) FROM briefcase.share_reviews r JOIN briefcase.accounts a ON a.account_uuid=r.granted_by_id
             WHERE r.principal_id=$1 AND briefcase.manages_share_review($1) AND a.status <> 'deleted'
               AND (r.expires_at IS NULL OR r.expires_at>clock_timestamp()) AND ($2::uuid IS NULL OR r.grant_id<$2)
             ORDER BY r.grant_id DESC LIMIT 51"
        ).bind(recipient).bind(cursor).fetch_all(&mut *tx).await.map_err(db)?;
        let next = if items.len() > 50 {
            items.truncate(50);
            items.last().map(|i| i["id"].clone())
        } else {
            None
        };
        let allowed: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('uuid',a.account_uuid,'id',a.public_id,'kind',a.kind,'display_name',a.display_name)
             FROM briefcase.share_review_senders s JOIN briefcase.accounts a ON a.account_uuid=s.sender
             WHERE s.recipient=$1 AND briefcase.manages_share_review($1) AND a.status <> 'deleted'
             ORDER BY a.public_id,a.account_uuid"
        ).bind(recipient).fetch_all(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(json!({"items":items,"next_cursor":next,"allowed_senders":allowed}))
    }

    pub(crate) async fn remove_review_sender(
        &self,
        ctx: &ExecutionContext,
        recipient: &str,
        sender: &str,
    ) -> Result<(), AppError> {
        let mut tx = self
            .begin(&TenantContext::from_execution(ctx))
            .await
            .map_err(db)?;
        lock_pair(&mut tx, recipient, sender)
            .await
            .map_err(repo_error)?;
        sqlx::query("DELETE FROM briefcase.share_review_senders WHERE recipient=$1 AND sender=$2 AND briefcase.manages_share_review($1)")
            .bind(recipient).bind(sender).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub(crate) async fn decide_share_review(
        &self,
        ctx: &ExecutionContext,
        recipient: &str,
        target: ReviewTarget,
    ) -> Result<(), AppError> {
        let mut tenant = TenantContext::from_execution(ctx);
        let mut tx = self.begin(&tenant).await.map_err(db)?;
        // Lock the account through commit so a concurrent custodian change cannot
        // transfer an in-flight decision to another manager.
        let managed: Option<String> = sqlx::query_scalar(
            "SELECT account_uuid FROM briefcase.accounts WHERE account_uuid=$1 AND briefcase.manages_share_review($1) FOR SHARE"
        ).bind(recipient).fetch_optional(&mut *tx).await.map_err(db)?;
        if managed.is_none() {
            return Err(AppError::NotFound);
        }
        let (id, sender, decision) = match target {
            ReviewTarget::Offer(id, decision) => {
                let sender: Option<String> = sqlx::query_scalar(
                    "SELECT granted_by_id FROM briefcase.share_reviews WHERE principal_id=$1 AND grant_id=$2
                     AND (expires_at IS NULL OR expires_at>clock_timestamp())"
                ).bind(recipient).bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
                let Some(sender) = sender else {
                    return Err(AppError::NotFound);
                };
                (Some(id), sender, decision)
            }
            ReviewTarget::Sender(sender) => (None, sender, ReviewDecision::AllowSender),
        };
        lock_pair(&mut tx, recipient, &sender)
            .await
            .map_err(repo_error)?;
        if decision == ReviewDecision::Dismiss {
            sqlx::query(
                "DELETE FROM briefcase.share_reviews WHERE principal_id=$1 AND grant_id=$2",
            )
            .bind(recipient)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
            return tx.commit().await.map_err(db);
        }
        let all = decision == ReviewDecision::AllowSender;
        let offers:Vec<(String,Uuid,Uuid)>=sqlx::query_as(
            "SELECT org_id,entry_id,grant_id FROM briefcase.share_reviews WHERE principal_id=$1 AND granted_by_id=$2
             AND ($3 OR grant_id=$4) AND (expires_at IS NULL OR expires_at>clock_timestamp()) ORDER BY org_id,entry_id,grant_id"
        ).bind(recipient).bind(&sender).bind(all).bind(id).fetch_all(&mut *tx).await.map_err(db)?;
        // The clicked offer must still exist before it can authorize trusting a sender.
        if id.is_some_and(|id| !offers.iter().any(|o| o.2 == id)) {
            return Err(AppError::NotFound);
        }
        if all {
            sqlx::query("INSERT INTO briefcase.share_review_senders(recipient,sender) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(recipient).bind(&sender).execute(&mut *tx).await.map_err(db)?;
        }
        for (drive, entry, offer) in offers {
            tenant.org_id = drive;
            sqlx::query("SELECT set_config('briefcase.org_id',$1,true)")
                .bind(tenant.org_id())
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            super::drives::add_share_recipient(&mut tx, ctx.authorization().actor())
                .await
                .map_err(repo_error)?;
            let delivered = deliver(&mut tx, &tenant, recipient, entry, offer).await?;
            if !delivered && decision == ReviewDecision::Accept {
                tx.commit().await.map_err(db)?;
                return Err(AppError::NotFound);
            }
        }
        tx.commit().await.map_err(db)
    }
}

pub(crate) enum ReviewTarget {
    Offer(Uuid, ReviewDecision),
    Sender(String),
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewDecision {
    Accept,
    AllowSender,
    Dismiss,
}

async fn deliver(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &TenantContext,
    recipient: &str,
    entry: Uuid,
    id: Uuid,
) -> Result<bool, AppError> {
    // File and drive owners, creators, and their current custodians are the
    // only accounts with ManagePermissions. A transfer, withdrawal or deletion
    // since the offer must never resurrect authority.
    let live:Option<(String,String,String)>=sqlx::query_as(
        "SELECT e.name,e.path,e.entry_type FROM briefcase.entries e
         JOIN briefcase.share_reviews r USING (org_id,entry_id)
         JOIN briefcase.accounts sender ON sender.account_uuid=r.granted_by_id
         WHERE e.org_id=briefcase.current_org_id() AND e.entry_id=$1 AND r.grant_id=$2 AND r.principal_id=$3
           AND e.deleted_at IS NULL AND e.system_kind IS NULL AND sender.status='active'
           AND (e.self_destruct_at IS NULL OR e.self_destruct_at>clock_timestamp())
           AND (r.granted_by_id IN (e.org_id,e.owner_id) OR EXISTS (
              SELECT FROM briefcase.accounts a WHERE a.account_uuid IN (e.org_id,e.owner_id)
                AND a.custodian_uuid=r.granted_by_id AND a.status<>'deleted'))
         FOR UPDATE OF e"
    ).bind(entry).bind(id).bind(recipient).fetch_optional(&mut **tx).await.map_err(db)?;
    let row:Option<(String,String,String,i16,bool,Option<time::OffsetDateTime>)>=sqlx::query_as(
        "DELETE FROM briefcase.share_reviews WHERE org_id=briefcase.current_org_id() AND grant_id=$1 AND principal_id=$2
         AND (expires_at IS NULL OR expires_at>clock_timestamp())
         RETURNING principal_type,granted_by_type,granted_by_id,access_mask,inherits_to_descendants,expires_at"
    ).bind(id).bind(recipient).fetch_optional(&mut **tx).await.map_err(db)?;
    let (
        Some((name, path, kind)),
        Some((principal_type, sender_type, sender, access, inherit, expiry)),
    ) = (live, row)
    else {
        return Ok(false);
    };
    let actual:Uuid=sqlx::query_scalar(
        "INSERT INTO briefcase.permission_grants(org_id,entry_id,grant_id,principal_type,principal_id,granted_by_type,
            granted_by_id,access_mask,inherits_to_descendants,expires_at)
         VALUES (briefcase.current_org_id(),$1,$2,$3,$4,$5,$6,$7,$8,$9)
         ON CONFLICT (org_id,entry_id,principal_type,principal_id) WHERE revoked_at IS NULL AND expires_at IS NULL
         DO UPDATE SET access_mask=EXCLUDED.access_mask,inherits_to_descendants=EXCLUDED.inherits_to_descendants,
            granted_by_type=EXCLUDED.granted_by_type,granted_by_id=EXCLUDED.granted_by_id RETURNING grant_id"
    ).bind(entry).bind(id).bind(&principal_type).bind(recipient).bind(&sender_type).bind(&sender)
        .bind(access).bind(inherit).bind(expiry).fetch_one(&mut **tx).await.map_err(db)?;
    if actual != id {
        sqlx::query("UPDATE briefcase.idempotency_records SET resource_id=$1 WHERE org_id=briefcase.current_org_id()
                     AND operation='grant_permission' AND actor_id=$2 AND resource_id=$3")
            .bind(actual).bind(&sender).bind(id).execute(&mut **tx).await.map_err(db)?;
    }
    record_change(tx,tenant,Some(entry),"permission.share_accepted.v1","entry",&entry.to_string(),
        json!({"grant_id":actual,"sender":sender,"principal":{"type":principal_type,"id":recipient},
            "access":common::decode_access(access).map_err(repo_error)?.rights().collect::<Vec<_>>(),
            "inherit":inherit,"expires_at":expiry.map(common::rfc3339)})).await.map_err(repo_error)?;
    sqlx::query("SELECT briefcase.deliver_notification($1,$2,'access_granted',$3,$4,$5,$6)")
        .bind(principal_type).bind(recipient).bind(sender_type).bind(sender).bind(entry)
        .bind(json!({"name":name,"path":path,"drive":tenant.org_id(),"entry_type":kind,"access_mask":access}))
        .execute(&mut **tx).await.map_err(db)?;
    Ok(true)
}
