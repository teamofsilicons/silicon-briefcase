//! Link authority and audit-log queries, always inside the selected tenant.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    PostgresRepository, TenantContext,
    metadata::common::{self, begin, load_entry, map_sql, record_change},
};
use crate::{
    application::{context::ExecutionContext, service::MutationMetadata},
    domain::{
        ids::EntryId,
        permission::{Capability, EntryVisibility},
    },
    error::AppError,
};

fn repo_error(value: crate::application::service::MetadataRepositoryError) -> AppError {
    crate::api::mapping::metadata_error(value.into())
}

fn error(value: super::metadata::common::Result<()>) -> Result<(), AppError> {
    value.map_err(repo_error)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LinkAccess {
    pub can_manage: bool,
    pub enabled: bool,
    pub effective: bool,
    pub inherited_from: Option<Uuid>,
}

impl PostgresRepository {
    pub(crate) async fn link_access(
        &self,
        context: &ExecutionContext,
        id: EntryId,
    ) -> Result<LinkAccess, AppError> {
        let mut request = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut request.transaction, context, id, false, false)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        if entry.authorization(context.authorization()).visibility() != EntryVisibility::Full {
            return Err(AppError::NotFound);
        }
        let safe_root = matches!(
            entry.system_kind,
            Some(
                crate::domain::entry::SystemEntryKind::PublicContainer
                    | crate::domain::entry::SystemEntryKind::TagRoot
            )
        );
        let can_manage = (!entry.entry.reserved
            && entry
                .authorization(context.authorization())
                .allows(Capability::ManagePermissions))
            || (safe_root && context.authorization().role().has_administrative_access());
        let result = read_link(&mut request.transaction, id.as_uuid(), can_manage).await?;
        request.transaction.commit().await.map_err(db)?;
        Ok(result)
    }

    pub(crate) async fn set_link_access(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        enabled: bool,
        metadata: &MutationMetadata,
    ) -> Result<LinkAccess, AppError> {
        let mut request = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut request.transaction, context, id, false, true)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        let auth = entry.authorization(context.authorization());
        if auth.visibility() != EntryVisibility::Full {
            return Err(AppError::NotFound);
        }
        let safe_root = matches!(
            entry.system_kind,
            Some(
                crate::domain::entry::SystemEntryKind::PublicContainer
                    | crate::domain::entry::SystemEntryKind::TagRoot
            )
        );
        if entry.entry.reserved && !safe_root {
            return Err(AppError::validation("protected_folder"));
        }
        if !(auth.allows(Capability::ManagePermissions)
            || safe_root && context.authorization().role().has_administrative_access())
        {
            return Err(AppError::Forbidden);
        }
        let claim = common::claim_idempotency(
            &mut request.transaction,
            &request.context,
            "set_link_access",
            metadata,
            Some(id.as_uuid()),
        )
        .await
        .map_err(repo_error)?;
        if !matches!(claim, common::IdempotencyClaim::Replay(_)) {
            sqlx::query("UPDATE briefcase.entries SET link_public=$2 WHERE org_id=briefcase.current_org_id() AND entry_id=$1")
                .bind(id.as_uuid()).bind(enabled).execute(&mut *request.transaction).await.map_err(db)?;
            error(
                record_change(
                    &mut request.transaction,
                    &request.context,
                    Some(id.as_uuid()),
                    "entry.link_access_changed.v1",
                    "entry",
                    &id.to_string(),
                    json!({"enabled":enabled}),
                )
                .await,
            )?;
            error(
                common::complete_idempotency(
                    &mut request.transaction,
                    &request.context,
                    "set_link_access",
                    metadata,
                    Some(id.as_uuid()),
                )
                .await,
            )?;
        }
        let result = read_link(&mut request.transaction, id.as_uuid(), true).await?;
        request.transaction.commit().await.map_err(db)?;
        Ok(result)
    }

    pub(crate) async fn logs(
        &self,
        context: &ExecutionContext,
        id: EntryId,
        cursor: Option<Uuid>,
        limit: u16,
    ) -> Result<LogPage, AppError> {
        let mut request = begin(self, context).await.map_err(repo_error)?;
        let entry = load_entry(&mut request.transaction, context, id, false, false)
            .await
            .map_err(repo_error)?
            .ok_or(AppError::NotFound)?;
        if !entry
            .authorization(context.authorization())
            .allows(Capability::Read)
        {
            return Err(AppError::NotFound);
        }
        let before: Option<OffsetDateTime> = if let Some(cursor) = cursor {
            Some(sqlx::query_scalar("SELECT occurred_at FROM briefcase.audit_events WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND audit_id=$2")
                .bind(id.as_uuid()).bind(cursor).fetch_optional(&mut *request.transaction).await.map_err(db)?.ok_or_else(|| AppError::bad_request("invalid_cursor"))?)
        } else {
            None
        };
        let mut rows = sqlx::query_as::<_, LogEvent>("SELECT audit_id AS id,actor_type,actor_id,origin_app_id AS app_id,action,metadata,occurred_at FROM briefcase.audit_events WHERE org_id=briefcase.current_org_id() AND entry_id=$1 AND occurred_at >= clock_timestamp()-interval '365 days' AND ($2::timestamptz IS NULL OR (occurred_at,audit_id)<($2,$3)) ORDER BY occurred_at DESC,audit_id DESC LIMIT $4")
            .bind(id.as_uuid()).bind(before).bind(cursor).bind(i64::from(limit)+1).fetch_all(&mut *request.transaction).await.map_err(db)?;
        let more = rows.len() > usize::from(limit);
        rows.truncate(usize::from(limit));
        let next_cursor = if more {
            rows.last().map(|row| row.id)
        } else {
            None
        };
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            if row.action.starts_with("child.")
                && !entry
                    .authorization(context.authorization())
                    .allows(Capability::ManagePermissions)
            {
                let child = row
                    .metadata
                    .get("entry_id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .and_then(|id| EntryId::from_uuid(id).ok());
                let Some(child) = child else {
                    continue;
                };
                let visible = load_entry(&mut request.transaction, context, child, false, false)
                    .await
                    .map_err(repo_error)?;
                if !visible.is_some_and(|e| {
                    e.authorization(context.authorization())
                        .allows(Capability::Read)
                }) {
                    continue;
                }
            }
            items.push(row);
        }
        request.transaction.commit().await.map_err(db)?;
        Ok(LogPage { items, next_cursor })
    }

    pub(crate) async fn public_entry(
        &self,
        tenant: &TenantContext,
        path: &str,
    ) -> Result<PublicEntry, AppError> {
        let mut tx = self.begin(tenant).await.map_err(db)?;
        let entry = public_entry(&mut tx, path).await?;
        tx.commit().await.map_err(db)?;
        Ok(entry)
    }

    pub(crate) async fn public_children(
        &self,
        tenant: &TenantContext,
        path: &str,
        cursor: Option<Uuid>,
    ) -> Result<PublicPage, AppError> {
        let mut tx = self.begin(tenant).await.map_err(db)?;
        let root = public_entry(&mut tx, path).await?;
        if root.entry_type != "folder" {
            return Err(AppError::NotFound);
        }
        let mut items = sqlx::query_as::<_, PublicEntry>("SELECT entry_id AS id,name,path,entry_type,content_type,size_bytes AS size FROM briefcase.entries WHERE org_id=briefcase.current_org_id() AND parent_id=$1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR entry_id<$2) ORDER BY entry_id DESC LIMIT 101")
            .bind(root.id).bind(cursor).fetch_all(&mut *tx).await.map_err(db)?;
        let more = items.len() > 100;
        items.truncate(100);
        let next_cursor = if more {
            items.last().map(|e| e.id)
        } else {
            None
        };
        tx.commit().await.map_err(db)?;
        Ok(PublicPage { items, next_cursor })
    }
}

pub(super) async fn public_entry(
    tx: &mut Transaction<'_, Postgres>,
    path: &str,
) -> Result<PublicEntry, AppError> {
    sqlx::query_as::<_, PublicEntry>("SELECT e.entry_id AS id,e.name,e.path,e.entry_type,e.content_type,e.size_bytes AS size FROM briefcase.entries e JOIN briefcase.organizations o ON o.org_id=e.org_id WHERE e.org_id=briefcase.current_org_id() AND o.lifecycle_status='active' AND e.path=$1 AND e.deleted_at IS NULL AND EXISTS(SELECT 1 FROM briefcase.entry_closure c JOIN briefcase.entries a ON a.org_id=c.org_id AND a.entry_id=c.ancestor_id WHERE c.org_id=e.org_id AND c.descendant_id=e.entry_id AND a.link_public AND a.deleted_at IS NULL) AND NOT EXISTS(SELECT 1 FROM briefcase.entry_closure c JOIN briefcase.entries a ON a.org_id=c.org_id AND a.entry_id=c.ancestor_id WHERE c.org_id=e.org_id AND c.descendant_id=e.entry_id AND a.deleted_at IS NOT NULL)")
        .bind(path).fetch_optional(&mut **tx).await.map_err(db)?.ok_or(AppError::NotFound)
}

async fn read_link(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    can_manage: bool,
) -> Result<LinkAccess, AppError> {
    let enabled = sqlx::query_scalar("SELECT link_public FROM briefcase.entries WHERE org_id=briefcase.current_org_id() AND entry_id=$1")
        .bind(id).fetch_one(&mut **tx).await.map_err(db)?;
    let inherited_from: Option<Uuid> = sqlx::query_scalar("SELECT a.entry_id FROM briefcase.entry_closure c JOIN briefcase.entries a ON a.org_id=c.org_id AND a.entry_id=c.ancestor_id WHERE c.org_id=briefcase.current_org_id() AND c.descendant_id=$1 AND c.depth>0 AND a.link_public AND a.deleted_at IS NULL ORDER BY c.depth LIMIT 1")
        .bind(id).fetch_optional(&mut **tx).await.map_err(db)?;
    Ok(LinkAccess {
        can_manage,
        enabled,
        effective: enabled || inherited_from.is_some(),
        inherited_from,
    })
}

pub(super) fn db(error: sqlx::Error) -> AppError {
    repo_error(map_sql(error))
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub(crate) struct PublicEntry {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub entry_type: String,
    pub content_type: Option<String>,
    pub size: Option<i64>,
}
#[derive(Serialize)]
pub(crate) struct PublicPage {
    pub items: Vec<PublicEntry>,
    pub next_cursor: Option<Uuid>,
}
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct LogEvent {
    pub id: Uuid,
    pub actor_type: String,
    pub actor_id: String,
    pub app_id: Option<String>,
    pub action: String,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
}
#[derive(Serialize)]
pub(crate) struct LogPage {
    pub items: Vec<LogEvent>,
    pub next_cursor: Option<Uuid>,
}
