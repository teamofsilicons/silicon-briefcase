//! Dynamic tag invitations and IAM-verified recipient contacts.
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
        actor::ActorRef,
        entry::EntryKind,
        ids::{EntryId, GrantId},
        permission::{AccessRight, Capability, GrantedAccess},
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
    pub(crate) async fn record_self_contact(
        &self,
        context: &ExecutionContext,
        email: &str,
    ) -> Result<(), AppError> {
        if !valid_email(email) {
            return Err(AppError::validation("invalid_email"));
        }
        let mut r = begin(self, context).await.map_err(repo_error)?;
        let actor = context.authorization().actor();
        sqlx::query("INSERT INTO briefcase.member_contacts(org_id,actor_type,actor_id,email) VALUES(briefcase.current_org_id(),$1,$2,$3) ON CONFLICT(org_id,actor_type,actor_id) DO UPDATE SET email=EXCLUDED.email,verified_at=clock_timestamp()")
            .bind(actor_kind(actor.kind())).bind(actor.id().as_str()).bind(email).execute(&mut *r.transaction).await.map_err(db)?;
        r.transaction.commit().await.map_err(db)
    }
    pub(crate) async fn member_by_email(
        &self,
        context: &ExecutionContext,
        email: &str,
    ) -> Result<ActorRef, AppError> {
        if !valid_email(email) {
            return Err(AppError::validation("invalid_email"));
        }
        let mut r = begin(self, context).await.map_err(repo_error)?;
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT c.actor_type,c.actor_id FROM briefcase.member_contacts c JOIN briefcase.organization_members m USING(org_id,actor_type,actor_id) WHERE c.org_id=briefcase.current_org_id() AND lower(c.email)=lower($1) AND m.membership_status='active' LIMIT 2")
            .bind(email).fetch_all(&mut *r.transaction).await.map_err(db)?;
        if rows.len() != 1 {
            return Err(AppError::validation("email_not_in_organization_directory"));
        }
        let actor = common::actor_ref(&rows[0].0, &rows[0].1).map_err(repo_error)?;
        r.transaction.commit().await.map_err(db)?;
        Ok(actor)
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn invite_tag(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        tag: &str,
        access: GrantedAccess,
        inherit: bool,
        lifetime: Option<LifetimeMinutes>,
        metadata: &MutationMetadata,
    ) -> Result<(Uuid, Option<OffsetDateTime>), AppError> {
        let mut r = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut r.transaction, context, id, false, true)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        let auth = entry.authorization(context.authorization());
        if !auth.allows(Capability::Read) {
            return Err(AppError::NotFound);
        }
        if !auth.allows(Capability::ManagePermissions) {
            return Err(AppError::Forbidden);
        }
        if access.contains(AccessRight::Delete)
            || (entry.entry.kind == EntryKind::File && access.contains(AccessRight::Write))
        {
            return Err(AppError::validation("invalid_invitation_rights"));
        }
        if lifetime.is_some() && access.rights().any(|right| right != AccessRight::Read) {
            return Err(AppError::validation("expiring_share_is_read_only"));
        }
        let tag_id:String=sqlx::query_scalar("SELECT tag_id FROM briefcase.organization_tags WHERE org_id=briefcase.current_org_id() AND (tag_id=$1 OR name=$1) AND lifecycle_status='active' ORDER BY (tag_id=$1) DESC LIMIT 1")
            .bind(tag).fetch_optional(&mut *r.transaction).await.map_err(db)?.ok_or_else(||AppError::validation("invalid_tag"))?;
        let claim = common::claim_idempotency(
            &mut r.transaction,
            &r.context,
            "invite_tag",
            metadata,
            Some(Uuid::now_v7()),
        )
        .await
        .map_err(repo_error)?;
        let proposed = match claim {
            common::IdempotencyClaim::Replay(Some(id)) => {
                let expires_at: Option<OffsetDateTime> = sqlx::query_scalar("SELECT expires_at FROM briefcase.tag_permission_grants WHERE org_id=briefcase.current_org_id() AND grant_id=$1")
                    .bind(id).fetch_optional(&mut *r.transaction).await.map_err(db)?.flatten();
                r.transaction.commit().await.map_err(db)?;
                return Ok((id, expires_at));
            }
            common::IdempotencyClaim::Acquired(Some(id)) => id,
            _ => return Err(AppError::conflict("invalid_invitation_state")),
        };
        let actor = context.authorization().actor();
        // An expiring share is its own row; only a permanent grant amends in place.
        let (grant, expires_at): (Uuid, Option<OffsetDateTime>) = if let Some(lifetime) = lifetime {
            sqlx::query_as("INSERT INTO briefcase.tag_permission_grants(org_id,entry_id,grant_id,tag_id,access_mask,inherits_to_descendants,granted_by_type,granted_by_id,expires_at) VALUES(briefcase.current_org_id(),$1,$2,$3,$4,$5,$6,$7,clock_timestamp()+make_interval(mins=>$8)) RETURNING grant_id,expires_at")
                .bind(id.as_uuid()).bind(proposed).bind(&tag_id).bind(common::encode_access(access)).bind(inherit).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).bind(lifetime.as_i32()).fetch_one(&mut *r.transaction).await.map_err(db)?
        } else {
            sqlx::query_as("INSERT INTO briefcase.tag_permission_grants(org_id,entry_id,grant_id,tag_id,access_mask,inherits_to_descendants,granted_by_type,granted_by_id) VALUES(briefcase.current_org_id(),$1,$2,$3,$4,$5,$6,$7) ON CONFLICT(org_id,entry_id,tag_id) WHERE revoked_at IS NULL AND expires_at IS NULL DO UPDATE SET access_mask=EXCLUDED.access_mask,inherits_to_descendants=EXCLUDED.inherits_to_descendants RETURNING grant_id,expires_at")
                .bind(id.as_uuid()).bind(proposed).bind(&tag_id).bind(common::encode_access(access)).bind(inherit).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).fetch_one(&mut *r.transaction).await.map_err(db)?
        };
        record_change(
            &mut r.transaction,
            &r.context,
            Some(id.as_uuid()),
            if expires_at.is_some() {
                "permission.tag_expiring_share_granted.v1"
            } else {
                "permission.tag_granted.v1"
            },
            "entry",
            &id.to_string(),
            json!({"grant_id":grant,"tag_id":tag_id,"access":access.rights().collect::<Vec<_>>(),"inherit":inherit,"expires_at":expires_at.map(common::rfc3339)}),
        )
        .await
        .map_err(repo_error)?;
        notify_tag(
            &mut r.transaction,
            context,
            &entry.entry,
            &tag_id,
            "access_granted",
            Some(access),
        )
        .await?;
        common::complete_idempotency(
            &mut r.transaction,
            &r.context,
            "invite_tag",
            metadata,
            Some(grant),
        )
        .await
        .map_err(repo_error)?;
        r.transaction.commit().await.map_err(db)?;
        Ok((grant, expires_at))
    }
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
        let mut items:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',grant_id,'principal',jsonb_build_object('type',principal_type,'id',principal_id),'access',array_remove(ARRAY['read',CASE WHEN access_mask & 2 <> 0 THEN 'write' END,CASE WHEN access_mask & 4 <> 0 THEN 'update' END],NULL),'inherit',inherits_to_descendants,'expires_at',expires_at) FROM (SELECT grant_id,principal_type,principal_id,access_mask,inherits_to_descendants,expires_at FROM briefcase.permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) UNION ALL SELECT grant_id,'tag',tag_id,access_mask,inherits_to_descendants,expires_at FROM briefcase.tag_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp())) grants WHERE ($2::uuid IS NULL OR grant_id > $2) ORDER BY grant_id LIMIT 101")
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
    pub(crate) async fn revoke_tag_or_member(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        grant: Uuid,
        metadata: &MutationMetadata,
    ) -> Result<(), AppError> {
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
        let tag:Option<(String,Option<OffsetDateTime>,bool)>=sqlx::query_as("SELECT tag_id,expires_at,COALESCE(expires_at<=clock_timestamp(),false) FROM briefcase.tag_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2").bind(id.as_uuid()).bind(grant).fetch_optional(&mut *r.transaction).await.map_err(db)?;
        if let Some((tag, expires_at, expired)) = tag {
            if matches!(
                common::claim_idempotency(
                    &mut r.transaction,
                    &r.context,
                    "revoke_tag",
                    metadata,
                    None
                )
                .await
                .map_err(repo_error)?,
                common::IdempotencyClaim::Replay(_)
            ) {
                return r.transaction.commit().await.map_err(db);
            }
            // An expired expiring share is already gone.
            if expired {
                return Err(AppError::NotFound);
            }
            let actor = context.authorization().actor();
            let changed=sqlx::query("UPDATE briefcase.tag_permission_grants SET revoked_at=clock_timestamp(),revoked_by_type=$3,revoked_by_id=$4 WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 AND revoked_at IS NULL").bind(id.as_uuid()).bind(grant).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).execute(&mut *r.transaction).await.map_err(db)?;
            if changed.rows_affected() > 0 {
                record_change(
                    &mut r.transaction,
                    &r.context,
                    Some(id.as_uuid()),
                    if expires_at.is_some() {
                        "permission.tag_expiring_share_revoked.v1"
                    } else {
                        "permission.tag_revoked.v1"
                    },
                    "entry",
                    &id.to_string(),
                    json!({"grant_id":grant,"tag_id":tag}),
                )
                .await
                .map_err(repo_error)?;
            }
            // An expiring share ends without a notification.
            if changed.rows_affected() > 0 && expires_at.is_none() {
                notify_tag(
                    &mut r.transaction,
                    context,
                    &entry.entry,
                    &tag,
                    "access_revoked",
                    None,
                )
                .await?;
            }
            common::complete_idempotency(
                &mut r.transaction,
                &r.context,
                "revoke_tag",
                metadata,
                None,
            )
            .await
            .map_err(repo_error)?;
            r.transaction.commit().await.map_err(db)
        } else {
            r.transaction.commit().await.map_err(db)?;
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
        // Member and tag grants share one identifier space. PostgreSQL cannot
        // lock through a UNION, so each table is locked on its own.
        let mut share: Option<ExpiringShareRow> = sqlx::query_as(
            "SELECT principal_type,principal_id,inherits_to_descendants,expires_at,false AS tag \
               FROM briefcase.permission_grants \
              WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 AND revoked_at IS NULL \
                AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR UPDATE",
        )
        .bind(id.as_uuid())
        .bind(grant)
        .fetch_optional(&mut *r.transaction)
        .await
        .map_err(db)?;
        if share.is_none() {
            share = sqlx::query_as(
                "SELECT 'tag' AS principal_type,tag_id AS principal_id,inherits_to_descendants,expires_at,true AS tag \
                   FROM briefcase.tag_permission_grants \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 AND revoked_at IS NULL \
                    AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR UPDATE",
            )
            .bind(id.as_uuid())
            .bind(grant)
            .fetch_optional(&mut *r.transaction)
            .await
            .map_err(db)?;
        }
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
        // Member and tag grants live in two tables with the same expiry
        // columns; every statement below exists once per table.
        let pick = |member: &'static str, tag: &'static str| if share.tag { tag } else { member };
        let actor = context.authorization().actor();
        let result = if let Some(lifetime) = lifetime {
            let expires_at: OffsetDateTime = sqlx::query_scalar(pick(
                "UPDATE briefcase.permission_grants SET expires_at=clock_timestamp()+make_interval(mins=>$3) \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 RETURNING expires_at",
                "UPDATE briefcase.tag_permission_grants SET expires_at=clock_timestamp()+make_interval(mins=>$3) \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2 RETURNING expires_at",
            ))
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
            let permanent: Option<Uuid> = sqlx::query_scalar(pick(
                "SELECT grant_id FROM briefcase.permission_grants \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND expires_at IS NULL \
                    AND principal_type=$2 AND principal_id=$3 FOR UPDATE",
                "SELECT grant_id FROM briefcase.tag_permission_grants \
                  WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND expires_at IS NULL \
                    AND $2::text IS NOT NULL AND tag_id=$3 FOR UPDATE",
            ))
            .bind(id.as_uuid())
            .bind(&share.principal_type)
            .bind(&share.principal_id)
            .fetch_optional(&mut *r.transaction)
            .await
            .map_err(db)?;
            let result = if let Some(permanent) = permanent {
                sqlx::query(pick(
                    "UPDATE briefcase.permission_grants SET inherits_to_descendants = inherits_to_descendants OR $3 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                    "UPDATE briefcase.tag_permission_grants SET inherits_to_descendants = inherits_to_descendants OR $3 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                ))
                .bind(id.as_uuid())
                .bind(permanent)
                .bind(share.inherits_to_descendants)
                .execute(&mut *r.transaction)
                .await
                .map_err(db)?;
                sqlx::query(pick(
                    "UPDATE briefcase.permission_grants SET revoked_at=clock_timestamp(),revoked_by_type=$3,revoked_by_id=$4 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                    "UPDATE briefcase.tag_permission_grants SET revoked_at=clock_timestamp(),revoked_by_type=$3,revoked_by_id=$4 \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                ))
                .bind(id.as_uuid())
                .bind(grant)
                .bind(actor_kind(actor.kind()))
                .bind(actor.id().as_str())
                .execute(&mut *r.transaction)
                .await
                .map_err(db)?;
                permanent
            } else {
                sqlx::query(pick(
                    "UPDATE briefcase.permission_grants SET expires_at=NULL \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                    "UPDATE briefcase.tag_permission_grants SET expires_at=NULL \
                      WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND grant_id=$2",
                ))
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
    tag: bool,
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

/// Live member and tag grants on one entry, rendered as invitation items.
/// `$1` is the entry; `$2`, when not null, selects one grant.
const INVITATION_JSON: &str = "SELECT jsonb_build_object('id',grant_id,'principal',jsonb_build_object('type',principal_type,'id',principal_id),'access',array_remove(ARRAY['read',CASE WHEN access_mask & 2 <> 0 THEN 'write' END,CASE WHEN access_mask & 4 <> 0 THEN 'update' END],NULL),'inherit',inherits_to_descendants,'expires_at',expires_at) FROM (SELECT grant_id,principal_type,principal_id,access_mask,inherits_to_descendants,expires_at FROM briefcase.permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) UNION ALL SELECT grant_id,'tag',tag_id,access_mask,inherits_to_descendants,expires_at FROM briefcase.tag_permission_grants WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp())) grants WHERE grant_id=$2";

fn valid_email(email: &str) -> bool {
    email.len() <= 254
        && email.bytes().filter(|b| *b == b'@').count() == 1
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
        && !email.bytes().any(|b| {
            b.is_ascii_whitespace()
                || b.is_ascii_control()
                || matches!(b, b',' | b';' | b'<' | b'>')
        })
}

/// Fanout is one transaction-local statement; the notification trigger queues mail.
async fn notify_tag(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    context: &ExecutionContext,
    entry: &crate::application::service::EntryView,
    tag: &str,
    kind: &str,
    access: Option<GrantedAccess>,
) -> Result<(), AppError> {
    let actor = context.authorization().actor();
    sqlx::query("INSERT INTO briefcase.notifications(org_id,notification_id,recipient_type,recipient_id,kind,actor_type,actor_id,entry_id,details) SELECT m.org_id,briefcase.new_uuid_v7(),m.actor_type,m.actor_id,$2,$3,$4,$5,$6 FROM briefcase.organization_members m JOIN briefcase.organization_member_tags t USING(org_id,actor_type,actor_id) WHERE m.org_id=briefcase.current_org_id() AND t.tag_id=$1 AND m.membership_status='active'")
        .bind(tag).bind(kind).bind(actor_kind(actor.kind())).bind(actor.id().as_str()).bind(entry.id.as_uuid())
        .bind(json!({"name":entry.name.as_str(),"path":entry.path.as_str(),"entry_type":if entry.kind==EntryKind::File {"file"} else {"folder"},"access_mask":access.map(common::encode_access)}))
        .execute(&mut **transaction).await.map_err(db)?;
    Ok(())
}
