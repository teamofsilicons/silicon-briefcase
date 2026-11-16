//! Dynamic organization/team invitations, under the same file ownership policy.
use super::{
    PostgresRepository,
    metadata::common::{self, actor_kind, begin, load_entry, record_change},
    sharing::db,
};
use crate::{
    application::{context::ExecutionContext, service::MutationMetadata},
    domain::{
        ids::EntryId,
        lifetime::LifetimeMinutes,
        permission::{AccessRight, Capability, GrantedAccess},
    },
    error::AppError,
    infrastructure::organizations::ShareTarget,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
fn repo_error(error: crate::application::service::MetadataRepositoryError) -> AppError {
    crate::api::mapping::metadata_error(error.into())
}

impl PostgresRepository {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn invite_group(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        target: &ShareTarget,
        access: GrantedAccess,
        inherit: bool,
        lifetime: Option<LifetimeMinutes>,
        metadata: &MutationMetadata,
    ) -> Result<Value, AppError> {
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
        if !access.contains(AccessRight::Read)
            || access.contains(AccessRight::Delete)
            || (!entry.entry.kind.can_contain_children() && access.contains(AccessRight::Write))
        {
            return Err(AppError::validation("invalid_invitation_rights"));
        }
        if lifetime.is_some() && access.rights().any(|right| right != AccessRight::Read) {
            return Err(AppError::validation("expiring_share_is_read_only"));
        }
        let claim = common::claim_idempotency(
            &mut r.transaction,
            &r.context,
            "invite_group",
            metadata,
            Some(Uuid::now_v7()),
        )
        .await
        .map_err(repo_error)?;
        let proposed = match claim {
            common::IdempotencyClaim::Replay(Some(grant)) => {
                let value = group_value(&mut r.transaction, id, grant).await?;
                r.transaction.commit().await.map_err(db)?;
                return Ok(value);
            }
            common::IdempotencyClaim::Acquired(Some(id)) => id,
            _ => return Err(AppError::conflict("invalid_invitation_state")),
        };
        let actor = context.authorization().actor();
        let grant: Uuid = sqlx::query_scalar(
            "INSERT INTO briefcase.group_permission_grants(org_id,entry_id,grant_id,principal_type,principal_id,display_name,access_mask,inherits_to_descendants,granted_by_type,granted_by_id,expires_at) VALUES(briefcase.current_org_id(),$1,$2,$3,$4,$5,$6,$7,$8,$9,CASE WHEN $10::integer IS NULL THEN NULL ELSE clock_timestamp()+make_interval(mins=>$10) END) ON CONFLICT(org_id,entry_id,principal_type,principal_id) WHERE revoked_at IS NULL AND expires_at IS NULL DO UPDATE SET access_mask=EXCLUDED.access_mask,inherits_to_descendants=EXCLUDED.inherits_to_descendants,display_name=EXCLUDED.display_name RETURNING grant_id"
        ).bind(id.as_uuid()).bind(proposed).bind(&target.kind).bind(&target.id).bind(&target.name).bind(common::encode_access(access)).bind(inherit).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).bind(lifetime.map(LifetimeMinutes::as_i32)).fetch_one(&mut *r.transaction).await.map_err(db)?;
        record_change(&mut r.transaction, &r.context, Some(id.as_uuid()), "permission.group_granted.v1", "entry", &id.to_string(), json!({"grant_id":grant,"type":target.kind,"group":target.id,"access":access.rights().collect::<Vec<_>>()})).await.map_err(repo_error)?;
        common::complete_idempotency(
            &mut r.transaction,
            &r.context,
            "invite_group",
            metadata,
            Some(grant),
        )
        .await
        .map_err(repo_error)?;
        let value = group_value(&mut r.transaction, id, grant).await?;
        r.transaction.commit().await.map_err(db)?;
        Ok(value)
    }

    /// Returns false when the grant is an ordinary account invitation.
    pub(crate) async fn revoke_group(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        grant: Uuid,
        metadata: &MutationMetadata,
    ) -> Result<bool, AppError> {
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
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM briefcase.group_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2)").bind(id.as_uuid()).bind(grant).fetch_one(&mut *r.transaction).await.map_err(db)?;
        if !exists {
            return Ok(false);
        }
        if !matches!(
            common::claim_idempotency(
                &mut r.transaction,
                &r.context,
                "revoke_group",
                metadata,
                None
            )
            .await
            .map_err(repo_error)?,
            common::IdempotencyClaim::Replay(_)
        ) {
            let actor = context.authorization().actor();
            let changed=sqlx::query("UPDATE briefcase.group_permission_grants SET revoked_at=clock_timestamp(),revoked_by_type=$3,revoked_by_id=$4 WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 AND revoked_at IS NULL").bind(id.as_uuid()).bind(grant).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).execute(&mut *r.transaction).await.map_err(db)?;
            if changed.rows_affected() > 0 {
                record_change(
                    &mut r.transaction,
                    &r.context,
                    Some(id.as_uuid()),
                    "permission.group_revoked.v1",
                    "entry",
                    &id.to_string(),
                    json!({"grant_id":grant}),
                )
                .await
                .map_err(repo_error)?;
            }
            common::complete_idempotency(
                &mut r.transaction,
                &r.context,
                "revoke_group",
                metadata,
                None,
            )
            .await
            .map_err(repo_error)?;
        }
        r.transaction.commit().await.map_err(db)?;
        Ok(true)
    }

    pub(crate) async fn change_group_expiry(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        grant: Uuid,
        lifetime: Option<LifetimeMinutes>,
        metadata: &MutationMetadata,
    ) -> Result<Option<Value>, AppError> {
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
        let row:Option<(Option<OffsetDateTime>,bool)>=sqlx::query_as("SELECT expires_at,revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FROM briefcase.group_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 FOR UPDATE").bind(id.as_uuid()).bind(grant).fetch_optional(&mut *r.transaction).await.map_err(db)?;
        let Some((expires, active)) = row else {
            return Ok(None);
        };
        if !active {
            return Err(AppError::NotFound);
        }
        if !matches!(
            common::claim_idempotency(
                &mut r.transaction,
                &r.context,
                "group_expiry",
                metadata,
                Some(grant)
            )
            .await
            .map_err(repo_error)?,
            common::IdempotencyClaim::Replay(_)
        ) {
            if expires.is_none() {
                return Err(AppError::validation("not_an_expiring_share"));
            }
            if lifetime.is_none() {
                // Keep independent permanent access rather than overwrite it.
                let duplicate:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM briefcase.group_permission_grants p JOIN briefcase.group_permission_grants e ON p.org_id=e.org_id AND p.entry_id=e.entry_id AND p.principal_type=e.principal_type AND p.principal_id=e.principal_id WHERE e.org_id=briefcase.current_org_id() AND e.grant_id=$1 AND p.grant_id<>e.grant_id AND p.revoked_at IS NULL AND p.expires_at IS NULL)").bind(grant).fetch_one(&mut *r.transaction).await.map_err(db)?;
                if duplicate {
                    return Err(AppError::conflict("group_already_permanent"));
                }
            }
            sqlx::query("UPDATE briefcase.group_permission_grants SET expires_at=CASE WHEN $3::integer IS NULL THEN NULL ELSE clock_timestamp()+make_interval(mins=>$3) END WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2").bind(id.as_uuid()).bind(grant).bind(lifetime.map(LifetimeMinutes::as_i32)).execute(&mut *r.transaction).await.map_err(db)?;
            record_change(
                &mut r.transaction,
                &r.context,
                Some(id.as_uuid()),
                "permission.group_expiry_changed.v1",
                "entry",
                &id.to_string(),
                json!({"grant_id":grant}),
            )
            .await
            .map_err(repo_error)?;
            common::complete_idempotency(
                &mut r.transaction,
                &r.context,
                "group_expiry",
                metadata,
                Some(grant),
            )
            .await
            .map_err(repo_error)?;
        }
        let value = group_value(&mut r.transaction, id, grant).await?;
        r.transaction.commit().await.map_err(db)?;
        Ok(Some(value))
    }
}

async fn group_value(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    entry: EntryId,
    grant: Uuid,
) -> Result<Value, AppError> {
    sqlx::query_scalar("SELECT jsonb_build_object('id',grant_id,'principal',jsonb_build_object('type',principal_type,'id',principal_id,'name',display_name),'access',array_remove(ARRAY['read',CASE WHEN access_mask & 2<>0 THEN 'write' END,CASE WHEN access_mask & 4<>0 THEN 'update' END],NULL),'inherit',inherits_to_descendants,'expires_at',expires_at) FROM briefcase.group_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2")
        .bind(entry.as_uuid()).bind(grant).fetch_optional(&mut **tx).await.map_err(db)?.ok_or(AppError::NotFound)
}
