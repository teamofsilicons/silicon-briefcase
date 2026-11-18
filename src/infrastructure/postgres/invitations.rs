//! Invitation listing, revocation and expiring-share changes, for account and organization/team shares.
use super::{
    PostgresRepository,
    metadata::common::{self, actor_kind, begin, load_entry, record_change},
    sharing::db,
};
use crate::domain::lifetime::LifetimeMinutes;
use crate::{
    application::{
        context::ExecutionContext,
        service::{MetadataRepository, MutationMetadata, RevokePermissionCommand},
    },
    domain::{
        ids::{EntryId, GrantId},
        permission::Capability,
    },
    error::AppError,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
fn repo_error(e: crate::application::service::MetadataRepositoryError) -> AppError {
    crate::api::mapping::metadata_error(e.into())
}

impl PostgresRepository {
    pub(crate) async fn invitations(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        cursor: Option<Uuid>,
    ) -> Result<Value, AppError> {
        let mut r = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut r.transaction, context, id, false, false)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        if !entry
            .authorization(context.authorization())
            .allows(Capability::ManagePermissions)
        {
            return Err(AppError::NotFound);
        }
        let mut items: Vec<Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('id',grant_id,'principal',jsonb_build_object('type',principal_type,
                'id',principal_id,'name',display_name),'access',array_remove(ARRAY['read',
                CASE WHEN access_mask & 2 <> 0 THEN 'write' END,CASE WHEN access_mask & 4 <> 0 THEN 'update' END],NULL),
                'inherit',inherits_to_descendants,'expires_at',expires_at) ||
                CASE WHEN pending THEN jsonb_build_object('pending_review',true) ELSE '{}'::jsonb END
             FROM (
                SELECT org_id,entry_id,grant_id,principal_type,principal_id,NULL::text AS display_name,
                       access_mask,inherits_to_descendants,expires_at,false AS pending
                FROM briefcase.permission_grants WHERE revoked_at IS NULL
                UNION ALL SELECT org_id,entry_id,grant_id,principal_type,principal_id,display_name,
                       access_mask,inherits_to_descendants,expires_at,false FROM briefcase.group_permission_grants WHERE revoked_at IS NULL
                UNION ALL SELECT org_id,entry_id,grant_id,principal_type,principal_id,NULL,
                       access_mask,inherits_to_descendants,expires_at,true FROM briefcase.share_reviews
             ) grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND (expires_at IS NULL OR expires_at>clock_timestamp())
                       AND ($2::uuid IS NULL OR grant_id>$2) ORDER BY grant_id LIMIT 101")
            .bind(id.as_uuid()).bind(cursor).fetch_all(&mut *r.transaction).await.map_err(db)?;
        r.transaction.commit().await.map_err(db)?;
        let next_cursor = if items.len() > 100 {
            items.truncate(100);
            items.last().and_then(|item| item.get("id")).cloned()
        } else {
            None
        };
        Ok(json!({"items":items,"next_cursor":next_cursor}))
    }
    pub(crate) async fn revoke_invitation(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        grant: Uuid,
        metadata: &MutationMetadata,
    ) -> Result<(), AppError> {
        if self.revoke_group(context, id, grant, metadata).await? {
            return Ok(());
        }
        self.revoke_permission(
            context,
            RevokePermissionCommand {
                entry_id: id,
                grant_id: GrantId::from_uuid(grant).map_err(|_| AppError::NotFound)?,
            },
            metadata,
            Capability::ManagePermissions,
        )
        .await
        .map_err(repo_error)
    }

    /// Extends, shortens, or makes permanent one live expiring share.
    ///
    /// `lifetime` restarts the share's clock from now; `None` makes it
    /// permanent. When the principal already holds a permanent grant here, the
    /// expiring share folds into it (the permanent grant keeps its rights and gains
    /// the share's inheritance) instead of creating a second permanent row.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn change_expiring_share(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        grant: Uuid,
        lifetime: Option<LifetimeMinutes>,
        metadata: &MutationMetadata,
    ) -> Result<Value, AppError> {
        const OPERATION: &str = "change_expiring_share";
        if let Some(value) = self
            .change_group_expiry(context, id, grant, lifetime, metadata)
            .await?
        {
            return Ok(value);
        }
        let mut r = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut r.transaction, context, id, false, true)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        if !entry
            .authorization(context.authorization())
            .allows(Capability::ManagePermissions)
        {
            return Err(AppError::NotFound);
        }
        let share: Option<ExpiringShareRow> = sqlx::query_as(
            "SELECT principal_type,principal_id,inherits_to_descendants,expires_at \
               FROM briefcase.permission_grants \
              WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 AND revoked_at IS NULL \
                AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR UPDATE",
        )
        .bind(id.as_uuid())
        .bind(grant)
        .fetch_optional(&mut *r.transaction)
        .await
        .map_err(db)?;
        let claim = common::claim_idempotency(
            &mut r.transaction,
            &r.context,
            OPERATION,
            metadata,
            Some(grant),
        )
        .await
        .map_err(repo_error)?;
        if let common::IdempotencyClaim::Replay(Some(result)) = claim {
            let current = share_json(&mut r.transaction, id, result).await?;
            r.transaction.commit().await.map_err(db)?;
            return current.ok_or(AppError::NotFound);
        }
        let share = share.ok_or(AppError::NotFound)?;
        let Some(previous) = share.expires_at else {
            return Err(AppError::conflict("not_an_expiring_share"));
        };
        let actor = context.authorization().actor();
        let result = if let Some(lifetime) = lifetime {
            let expires_at: OffsetDateTime = sqlx::query_scalar("UPDATE briefcase.permission_grants SET expires_at=clock_timestamp()+make_interval(mins=>$3) \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 RETURNING expires_at")
            .bind(id.as_uuid())
            .bind(grant)
            .bind(lifetime.as_i32())
            .fetch_one(&mut *r.transaction)
            .await
            .map_err(db)?;
            record_change(
                &mut r.transaction,
                &r.context,
                Some(id.as_uuid()),
                "permission.expiring_share_changed.v1",
                "entry",
                &id.to_string(),
                json!({"grant_id":grant,"principal":{"type":share.principal_type,"id":share.principal_id},"previous_expires_at":common::rfc3339(previous),"expires_at":common::rfc3339(expires_at)}),
            )
            .await
            .map_err(repo_error)?;
            grant
        } else {
            let permanent: Option<Uuid> = sqlx::query_scalar("SELECT grant_id FROM briefcase.permission_grants \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND expires_at IS NULL \
                    AND principal_type=$2 AND principal_id=$3 FOR UPDATE")
            .bind(id.as_uuid())
            .bind(&share.principal_type)
            .bind(&share.principal_id)
            .fetch_optional(&mut *r.transaction)
            .await
            .map_err(db)?;
            let result = if let Some(permanent) = permanent {
                sqlx::query("UPDATE briefcase.permission_grants SET inherits_to_descendants = inherits_to_descendants OR $3 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2")
                .bind(id.as_uuid())
                .bind(permanent)
                .bind(share.inherits_to_descendants)
                .execute(&mut *r.transaction)
                .await
                .map_err(db)?;
                sqlx::query("UPDATE briefcase.permission_grants SET revoked_at=clock_timestamp(),revoked_by_type=$3,revoked_by_id=$4 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2")
                .bind(id.as_uuid())
                .bind(grant)
                .bind(actor_kind(actor.kind()))
                .bind(actor.id().as_str())
                .execute(&mut *r.transaction)
                .await
                .map_err(db)?;
                permanent
            } else {
                sqlx::query(
                    "UPDATE briefcase.permission_grants SET expires_at=NULL \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                )
                .bind(id.as_uuid())
                .bind(grant)
                .execute(&mut *r.transaction)
                .await
                .map_err(db)?;
                grant
            };
            record_change(
                &mut r.transaction,
                &r.context,
                Some(id.as_uuid()),
                "permission.expiring_share_made_permanent.v1",
                "entry",
                &id.to_string(),
                json!({"grant_id":grant,"principal":{"type":share.principal_type,"id":share.principal_id},"previous_expires_at":common::rfc3339(previous),"permanent_grant_id":result}),
            )
            .await
            .map_err(repo_error)?;
            result
        };
        common::complete_idempotency(
            &mut r.transaction,
            &r.context,
            OPERATION,
            metadata,
            Some(result),
        )
        .await
        .map_err(repo_error)?;
        let current = share_json(&mut r.transaction, id, result).await?;
        r.transaction.commit().await.map_err(db)?;
        current.ok_or(AppError::NotFound)
    }
}

#[derive(sqlx::FromRow)]
struct ExpiringShareRow {
    principal_type: String,
    principal_id: String,
    inherits_to_descendants: bool,
    expires_at: Option<OffsetDateTime>,
}

/// One live grant in the invitation listing's shape.
async fn share_json(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entry: EntryId,
    grant: Uuid,
) -> Result<Option<Value>, AppError> {
    sqlx::query_scalar(INVITATION_JSON)
        .bind(entry.as_uuid())
        .bind(grant)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(db)
}

/// Live account grants on one entry, rendered as invitation items.
/// `$1` is the entry; `$2`, when not null, selects one grant.
const INVITATION_JSON: &str = "SELECT jsonb_build_object('id',grant_id,'principal',jsonb_build_object('type',principal_type,'id',principal_id),'access',array_remove(ARRAY['read',CASE WHEN access_mask & 2 <> 0 THEN 'write' END,CASE WHEN access_mask & 4 <> 0 THEN 'update' END],NULL),'inherit',inherits_to_descendants,'expires_at',expires_at) FROM (SELECT grant_id,principal_type,principal_id,access_mask,inherits_to_descendants,expires_at FROM briefcase.permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp())) grants WHERE grant_id=$2";
