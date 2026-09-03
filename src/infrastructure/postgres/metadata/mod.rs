//! Concrete tenant-safe implementation of metadata application ports.

pub(super) mod common;
pub(super) mod filter;
pub(super) mod notifications;

use filter as filter_sql;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, QueryBuilder, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::{
        context::ExecutionContext,
        service::{
            AccessRequestView, ActivityEvent, AuthorizableAccessRequest, AuthorizableEntry,
            CreateFolderMutation, DecideAccessRequestCommand, ENTRY_ACTIVITY_HISTORY_SIZE,
            FileVersionView, GrantPermissionCommand, ListBinQuery, ListEntriesQuery,
            ListPermissionsQuery, ListVersionsQuery, MetadataRepository, MetadataRepositoryError,
            MutationMetadata, Page, ProjectedMembership, RequestAccessCommand,
            RevokePermissionCommand, SearchCandidate, SearchQuery, UpdateEntryCommand,
        },
    },
    domain::{
        access::{AccessDecision, AccessRequestStatus},
        actor::{ActorRef, ApplicationId, OrganizationId, OrganizationRole, TagName},
        entry::{EntryKind, EntryPath},
        filter::{FilterQuery, SortOrder},
        ids::{AccessRequestId, EntryId, GrantId, VersionId},
        notification::{NotificationDecision, NotificationInbox, NotificationKind},
        permission::{AccessRight, Capability, GrantedAccess, PermissionGrant},
        quota::OrganizationUsage,
        version::{VersionNumber, VersionSource},
    },
};

use super::{
    AccessRequestRow, EntryRow, PermissionGrantRow, PostgresRepository, TenantContext,
    models::entry_columns, roots,
};
use common::{
    IdempotencyClaim, Result, actor_kind, actor_ref, begin, boundary_columns, build_authorizable,
    claim_idempotency, complete_idempotency, current_member, decode_access, encode_access,
    internal, load_entry, map_sql, permission_grant, record_change, resolve_tag_id,
    retention_deadline,
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct UuidCursor {
    id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct NumberCursor {
    number: i64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct BinCursor {
    deleted_at: OffsetDateTime,
    id: Uuid,
}

/// Cursor for a chronological listing, which orders by last change.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct ChangeCursor {
    changed_at: OffsetDateTime,
    id: Uuid,
}

#[derive(sqlx::FromRow)]
struct SubtreeRow {
    entry_id: Uuid,
    depth: i32,
}

#[allow(clippy::too_many_lines)]
#[async_trait]
impl MetadataRepository for PostgresRepository {
    async fn find_active_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Option<AuthorizableEntry>> {
        let mut request = begin(self, context).await?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, false).await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entry)
    }

    async fn find_boundary_container(
        &self,
        context: &ExecutionContext,
        boundary: &crate::domain::entry::EntryBoundary,
    ) -> Result<Option<AuthorizableEntry>> {
        let mut request = begin(self, context).await?;
        let actor = context.authorization().actor();
        let row = sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() \
                AND deleted_at IS NULL \
                AND ( \
                    ($1 = 'public' AND system_kind = 'public_root') \
                    OR ($1 = 'private' AND system_kind = 'actor_root' \
                        AND owner_type = $3 AND owner_id = $4) \
                    OR ($1 = 'tag' AND system_kind = 'tag_root' AND tag_id IN ( \
                            SELECT tag_id FROM briefcase.organization_tags \
                             WHERE org_id = briefcase.current_org_id() \
                               AND name = $2 AND lifecycle_status = 'active' \
                        )) \
                ) \
              LIMIT 1",
        ))
        .bind(match boundary.root_type() {
            crate::domain::entry::RootType::Public => "public",
            crate::domain::entry::RootType::Private => "private",
            crate::domain::entry::RootType::Tag => "tag",
        })
        .bind(boundary.tag().map(crate::domain::actor::TagName::as_str))
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .fetch_optional(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let entry = match row {
            Some(row) => Some(build_authorizable(&mut request.transaction, context, row).await?),
            None => None,
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entry)
    }

    async fn find_active_entry_by_path(
        &self,
        context: &ExecutionContext,
        path: &EntryPath,
    ) -> Result<Option<AuthorizableEntry>> {
        let mut request = begin(self, context).await?;
        let row = sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() \
                AND path = $1 AND deleted_at IS NULL",
        ))
        .bind(path.as_str())
        .fetch_optional(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let entry = match row {
            Some(row) => Some(build_authorizable(&mut request.transaction, context, row).await?),
            None => None,
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entry)
    }

    async fn find_active_entries(
        &self,
        context: &ExecutionContext,
        entry_ids: &[EntryId],
        paths: &[EntryPath],
    ) -> Result<Vec<AuthorizableEntry>> {
        if entry_ids.is_empty() && paths.is_empty() {
            return Ok(Vec::new());
        }
        let identifiers: Vec<Uuid> = entry_ids.iter().map(|id| id.as_uuid()).collect();
        let paths: Vec<&str> = paths.iter().map(EntryPath::as_str).collect();
        let mut request = begin(self, context).await?;
        let rows = sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() \
                AND deleted_at IS NULL \
                AND (entry_id = ANY($1) OR path = ANY($2)) \
              ORDER BY path COLLATE \"C\"",
        ))
        .bind(&identifiers)
        .bind(&paths)
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            entries.push(build_authorizable(&mut request.transaction, context, row).await?);
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entries)
    }

    async fn list_active_children(
        &self,
        context: &ExecutionContext,
        query: &ListEntriesQuery,
    ) -> Result<Page<AuthorizableEntry>> {
        let filter = query.filter.as_ref();
        let scan = filter.map_or(SortOrder::Newest, FilterQuery::scan_order);
        let cursor = decode_optional::<ChangeCursor>(query.page.cursor.as_deref())?;
        // A chronological cap answers in one page, so it never carries a cursor.
        let take = filter.and_then(|filter| filter.take);
        let page_size = take.map_or(query.page.limit, |take| take.count.min(query.page.limit));
        let mut builder = QueryBuilder::<Postgres>::new(concat!("SELECT ", entry_columns!()));
        builder.push(
            " FROM briefcase.entries AS entry \
              WHERE entry.org_id = briefcase.current_org_id() \
                AND entry.deleted_at IS NULL",
        );
        // Without a filter the listing browses one level. With one, an
        // unspecified parent widens the scan to the whole organization so a
        // `location:` predicate can reach anywhere the caller may look.
        if let Some(parent_id) = query.parent_id {
            builder.push(" AND entry.parent_id = ");
            builder.push_bind(parent_id.as_uuid());
        } else if filter.is_none() {
            builder.push(" AND entry.parent_id IS NULL");
        }
        if let Some(cursor) = cursor {
            builder.push(" AND (entry.updated_at, entry.entry_id) ");
            builder.push(match scan {
                SortOrder::Newest => "< (",
                SortOrder::Oldest => "> (",
            });
            builder.push_bind(cursor.changed_at);
            builder.push(", ");
            builder.push_bind(cursor.id);
            builder.push(")");
        }
        if let Some(expression) = filter.and_then(|filter| filter.expression.as_ref()) {
            builder.push(" AND ");
            filter_sql::push_expression(&mut builder, expression);
        }
        builder.push(match scan {
            SortOrder::Newest => " ORDER BY entry.updated_at DESC, entry.entry_id DESC LIMIT ",
            SortOrder::Oldest => " ORDER BY entry.updated_at ASC, entry.entry_id ASC LIMIT ",
        });
        builder.push_bind(i64::from(page_size) + 1);

        let mut request = begin(self, context).await?;
        let rows = builder
            .build_query_as::<EntryRow>()
            .fetch_all(&mut *request.transaction)
            .await
            .map_err(map_sql)?;
        let has_more = take.is_none() && rows.len() > usize::from(page_size);
        let mut items = Vec::with_capacity(usize::from(page_size));
        for row in rows.into_iter().take(usize::from(page_size)) {
            items.push(build_authorizable(&mut request.transaction, context, row).await?);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|entry| {
                    encode_cursor(&ChangeCursor {
                        changed_at: entry.entry.updated_at,
                        id: entry.entry.id.as_uuid(),
                    })
                })
                .transpose()?
        } else {
            None
        };
        if filter.is_some_and(FilterQuery::requires_reversal) {
            items.reverse();
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(Page { items, next_cursor })
    }

    async fn create_folder(
        &self,
        context: &ExecutionContext,
        mutation: &CreateFolderMutation,
        metadata: &MutationMetadata,
        required_parent_capability: Option<Capability>,
    ) -> Result<AuthorizableEntry> {
        const OPERATION: &str = "create_folder";
        let mut request = begin(self, context).await?;
        if mutation.command.parent_id.is_none() {
            roots::lock_organization_reconciliation(&mut request.transaction)
                .await
                .map_err(map_sql)?;
        }
        let entry_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(mutation.entry_id.as_uuid()),
        )
        .await?
        {
            IdempotencyClaim::Replay(Some(resource_id)) => {
                let id = EntryId::from_uuid(resource_id).map_err(internal_data)?;
                let entry = load_entry(&mut request.transaction, context, id, false, false)
                    .await?
                    .ok_or(MetadataRepositoryError::Conflict)?;
                request.transaction.commit().await.map_err(map_sql)?;
                return Ok(entry);
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(MetadataRepositoryError::Conflict);
            }
            IdempotencyClaim::Acquired(Some(resource_id)) => {
                EntryId::from_uuid(resource_id).map_err(internal_data)?
            }
        };

        let actor = context.authorization().actor();
        if &mutation.owner != actor {
            return Err(MetadataRepositoryError::Conflict);
        }
        if let Some(parent_id) = mutation.command.parent_id {
            let parent = load_entry(&mut request.transaction, context, parent_id, false, true)
                .await?
                .ok_or(MetadataRepositoryError::NotFound)?;
            if parent.entry.kind != EntryKind::Folder || parent.entry.boundary != mutation.boundary
            {
                return Err(MetadataRepositoryError::Conflict);
            }
            let capability = required_parent_capability.ok_or(MetadataRepositoryError::Conflict)?;
            require_capability(&parent, context, capability)?;
        } else if required_parent_capability.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }

        let (root_type, tag_name) = boundary_columns(&mutation.boundary);
        let tag_id = resolve_tag_id(&mut request.transaction, tag_name).await?;
        sqlx::query(
            "INSERT INTO briefcase.entries ( \
                    org_id, entry_id, parent_id, entry_type, name, root_type, tag_id, \
                    owner_type, owner_id, origin_app_id, created_by_type, created_by_id, \
                    updated_by_type, updated_by_id \
             ) VALUES (briefcase.current_org_id(), $1, $2, 'folder', $3, $4, $5, \
                       $6, $7, $8, $6, $7, $6, $7)",
        )
        .bind(entry_id.as_uuid())
        .bind(mutation.command.parent_id.map(EntryId::as_uuid))
        .bind(mutation.command.name.as_str())
        .bind(root_type)
        .bind(tag_id.as_deref())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(
            mutation
                .origin_application_id
                .as_ref()
                .map(crate::domain::actor::ApplicationId::as_str),
        )
        .execute(&mut *request.transaction)
        .await
        .map_err(map_sql)?;

        for invitee in &mutation.command.invitees {
            if !current_member(&mut request.transaction, &invitee.principal).await? {
                return Err(MetadataRepositoryError::Conflict);
            }
            sqlx::query(
                "INSERT INTO briefcase.permission_grants ( \
                        org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                        inherits_to_descendants, granted_by_type, granted_by_id \
                 ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(entry_id.as_uuid())
            .bind(Uuid::now_v7())
            .bind(actor_kind(invitee.principal.kind()))
            .bind(invitee.principal.id().as_str())
            .bind(encode_access(invitee.access))
            .bind(invitee.inherits_to_descendants)
            .bind(actor_kind(actor.kind()))
            .bind(actor.id().as_str())
            .execute(&mut *request.transaction)
            .await
            .map_err(map_sql)?;
        }
        record_change(
            &mut request.transaction,
            &request.context,
            Some(entry_id.as_uuid()),
            "entry.folder_created.v1",
            "entry",
            &entry_id.to_string(),
            json!({"parent_id": mutation.command.parent_id.map(|id| id.to_string())}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(entry_id.as_uuid()),
        )
        .await?;
        let created = load_entry(&mut request.transaction, context, entry_id, false, false)
            .await?
            .ok_or_else(|| internal("created folder disappeared"))?;
        // Invitees learn about the folder from their inbox, in the same
        // transaction that gave them access to it.
        let subject = notifications::snapshot(&created.entry);
        for invitee in &mutation.command.invitees {
            notifications::insert(
                &mut request.transaction,
                &notifications::NewNotification {
                    recipient: &invitee.principal,
                    kind: NotificationKind::AccessGranted,
                    actor: Some(actor),
                    subject: Some(&subject),
                    access: Some(invitee.access),
                    access_request_id: None,
                    decision: None,
                },
            )
            .await?;
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(created)
    }

    async fn update_entry(
        &self,
        context: &ExecutionContext,
        command: &UpdateEntryCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AuthorizableEntry> {
        const OPERATION: &str = "update_entry";
        let mut request = begin(self, context).await?;
        // A rename can target an organization-root entry. Serialize metadata
        // mutations with system-root name reconciliation before taking row locks.
        roots::lock_organization_reconciliation(&mut request.transaction)
            .await
            .map_err(map_sql)?;
        let current = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await?
        .ok_or(MetadataRepositoryError::NotFound)?;
        require_capability(&current, context, required_capability)?;
        if current.system_kind.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }
        if let Some(parent_id) = command.parent_id {
            let parent = load_entry(&mut request.transaction, context, parent_id, false, true)
                .await?
                .ok_or(MetadataRepositoryError::NotFound)?;
            if parent.entry.kind != EntryKind::Folder
                || parent.entry.boundary != current.entry.boundary
            {
                return Err(MetadataRepositoryError::Conflict);
            }
            require_capability(&parent, context, Capability::CreateChild)?;
        }
        if command.parent_id.is_some() && current.entry.kind == EntryKind::Folder {
            lock_and_require_subtree_capability(
                &mut request.transaction,
                context,
                command.entry_id,
                Capability::UpdateMetadata,
            )
            .await?;
        }
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.entry_id.as_uuid()),
        )
        .await?
        {
            request.transaction.commit().await.map_err(map_sql)?;
            return Ok(current);
        }
        let actor = context.authorization().actor();
        sqlx::query(
            "UPDATE briefcase.entries \
                SET name = COALESCE($2, name), parent_id = COALESCE($3, parent_id), \
                    updated_by_type = $4, updated_by_id = $5 \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND deleted_at IS NULL",
        )
        .bind(command.entry_id.as_uuid())
        .bind(
            command
                .name
                .as_ref()
                .map(crate::domain::entry::EntryName::as_str),
        )
        .bind(command.parent_id.map(EntryId::as_uuid))
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.entry_id.as_uuid()),
            "entry.metadata_updated.v1",
            "entry",
            &command.entry_id.to_string(),
            json!({
                "renamed": command.name.is_some(),
                "moved": command.parent_id.is_some(),
            }),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.entry_id.as_uuid()),
        )
        .await?;
        let updated = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            false,
        )
        .await?
        .ok_or_else(|| internal("updated entry disappeared"))?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(updated)
    }

    async fn soft_delete_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<()> {
        const OPERATION: &str = "soft_delete_entry";
        let mut request = begin(self, context).await?;
        roots::lock_organization_reconciliation(&mut request.transaction)
            .await
            .map_err(map_sql)?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, true)
            .await?
            .ok_or(MetadataRepositoryError::NotFound)?;
        require_capability(&entry, context, required_capability)?;
        if entry.system_kind.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }
        lock_and_require_subtree_capability(
            &mut request.transaction,
            context,
            entry_id,
            Capability::Delete,
        )
        .await?;
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(entry_id.as_uuid()),
        )
        .await?
        {
            request.transaction.commit().await.map_err(map_sql)?;
            return Ok(());
        }
        let batch_id = Uuid::now_v7();
        let deleted_at = OffsetDateTime::now_utc();
        let purge_after = retention_deadline();
        let actor = context.authorization().actor();
        sqlx::query(
            "UPDATE briefcase.entries AS entry \
                SET deletion_batch_id = $2, deleted_at = $3, purge_after = $4, \
                    updated_by_type = $5, updated_by_id = $6 \
              WHERE entry.org_id = briefcase.current_org_id() \
                AND entry.deleted_at IS NULL \
                AND EXISTS ( \
                    SELECT 1 FROM briefcase.entry_closure AS subtree \
                     WHERE subtree.org_id = entry.org_id \
                       AND subtree.ancestor_id = $1 \
                       AND subtree.descendant_id = entry.entry_id \
                )",
        )
        .bind(entry_id.as_uuid())
        .bind(batch_id)
        .bind(deleted_at)
        .bind(purge_after)
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(entry_id.as_uuid()),
            "entry.subtree_deleted.v1",
            "entry",
            &entry_id.to_string(),
            json!({"deletion_batch_id": batch_id, "purge_after": purge_after}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(entry_id.as_uuid()),
        )
        .await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(())
    }

    async fn list_permission_grants(
        &self,
        context: &ExecutionContext,
        query: &ListPermissionsQuery,
    ) -> Result<Page<PermissionGrant>> {
        let cursor = decode_optional::<UuidCursor>(query.page.cursor.as_deref())?;
        let after = cursor.map_or(Uuid::nil(), |value| value.id);
        let mut request = begin(self, context).await?;
        let rows = sqlx::query_as::<_, PermissionGrantRow>(
            "SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                    inherits_to_descendants, granted_by_type, granted_by_id, revoked_at, \
                    revoked_by_type, revoked_by_id, created_at \
               FROM briefcase.permission_grants \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                AND revoked_at IS NULL AND grant_id > $2 \
              ORDER BY grant_id LIMIT $3",
        )
        .bind(query.entry_id.as_uuid())
        .bind(after)
        .bind(i64::from(query.page.limit) + 1)
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let has_more = rows.len() > usize::from(query.page.limit);
        let mut items = rows
            .into_iter()
            .take(usize::from(query.page.limit))
            .map(permission_grant)
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if has_more {
            items
                .last()
                .map(|grant| {
                    encode_cursor(&UuidCursor {
                        id: grant.id().as_uuid(),
                    })
                })
                .transpose()?
        } else {
            None
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(Page {
            items: std::mem::take(&mut items),
            next_cursor,
        })
    }

    async fn grant_permission(
        &self,
        context: &ExecutionContext,
        command: &GrantPermissionCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<PermissionGrant> {
        const OPERATION: &str = "grant_permission";
        let mut request = begin(self, context).await?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await?
        .ok_or(MetadataRepositoryError::NotFound)?;
        require_capability(&entry, context, required_capability)?;
        if entry.system_kind.is_some()
            || !current_member(&mut request.transaction, &command.principal).await?
        {
            return Err(MetadataRepositoryError::Conflict);
        }
        let proposed_grant_id = GrantId::new();
        let grant_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(proposed_grant_id.as_uuid()),
        )
        .await?
        {
            IdempotencyClaim::Replay(Some(id)) => {
                let row = find_grant(&mut request.transaction, id, false)
                    .await?
                    .ok_or(MetadataRepositoryError::Conflict)?;
                let grant = permission_grant(row)?;
                request.transaction.commit().await.map_err(map_sql)?;
                return Ok(grant);
            }
            IdempotencyClaim::Acquired(Some(id)) => {
                GrantId::from_uuid(id).map_err(internal_data)?
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(MetadataRepositoryError::Conflict);
            }
        };
        let actor = context.authorization().actor();
        let row = sqlx::query_as::<_, PermissionGrantRow>(
            // Granting a principal who already holds a grant amends it rather
            // than colliding with it: the contract has no operation that edits
            // a grant, so the alternative would be revoking access and issuing
            // it again, which briefly removes the access being widened and
            // tells the recipient their access was revoked.
            "INSERT INTO briefcase.permission_grants ( \
                    org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                    inherits_to_descendants, granted_by_type, granted_by_id \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (org_id, entry_id, principal_type, principal_id) \
                  WHERE revoked_at IS NULL \
             DO UPDATE SET access_mask = EXCLUDED.access_mask, \
                           inherits_to_descendants = EXCLUDED.inherits_to_descendants, \
                           granted_by_type = EXCLUDED.granted_by_type, \
                           granted_by_id = EXCLUDED.granted_by_id \
             RETURNING org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                       inherits_to_descendants, granted_by_type, granted_by_id, revoked_at, \
                       revoked_by_type, revoked_by_id, created_at",
        )
        .bind(command.entry_id.as_uuid())
        .bind(grant_id.as_uuid())
        .bind(actor_kind(command.principal.kind()))
        .bind(command.principal.id().as_str())
        .bind(encode_access(command.access))
        .bind(command.inherits_to_descendants)
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.entry_id.as_uuid()),
            "permission.granted.v1",
            "entry",
            &command.entry_id.to_string(),
            json!({"grant_id": grant_id, "access": access_rights(command.access)}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(grant_id.as_uuid()),
        )
        .await?;
        notifications::insert(
            &mut request.transaction,
            &notifications::NewNotification {
                recipient: &command.principal,
                kind: NotificationKind::AccessGranted,
                actor: Some(actor),
                subject: Some(&notifications::snapshot(&entry.entry)),
                access: Some(command.access),
                access_request_id: None,
                decision: None,
            },
        )
        .await?;
        let grant = permission_grant(row)?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(grant)
    }

    async fn revoke_permission(
        &self,
        context: &ExecutionContext,
        command: RevokePermissionCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<()> {
        const OPERATION: &str = "revoke_permission";
        let mut request = begin(self, context).await?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await?
        .ok_or(MetadataRepositoryError::NotFound)?;
        require_capability(&entry, context, required_capability)?;
        if entry.system_kind.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }
        let row = sqlx::query_as::<_, PermissionGrantRow>(
            "SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                    inherits_to_descendants, granted_by_type, granted_by_id, revoked_at, \
                    revoked_by_type, revoked_by_id, created_at \
               FROM briefcase.permission_grants \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND grant_id = $2 \
              FOR UPDATE",
        )
        .bind(command.entry_id.as_uuid())
        .bind(command.grant_id.as_uuid())
        .fetch_optional(&mut *request.transaction)
        .await
        .map_err(map_sql)?
        .ok_or(MetadataRepositoryError::NotFound)?;
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.grant_id.as_uuid()),
        )
        .await?
        {
            request.transaction.commit().await.map_err(map_sql)?;
            return Ok(());
        }
        if row.revoked_at.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }
        let actor = context.authorization().actor();
        sqlx::query(
            "UPDATE briefcase.permission_grants \
                SET revoked_at = clock_timestamp(), revoked_by_type = $3, revoked_by_id = $4 \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 AND grant_id = $2 \
                AND revoked_at IS NULL",
        )
        .bind(command.entry_id.as_uuid())
        .bind(command.grant_id.as_uuid())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .execute(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.entry_id.as_uuid()),
            "permission.revoked.v1",
            "entry",
            &command.entry_id.to_string(),
            json!({"grant_id": command.grant_id}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.grant_id.as_uuid()),
        )
        .await?;
        let principal = actor_ref(&row.principal_type, &row.principal_id)?;
        notifications::insert(
            &mut request.transaction,
            &notifications::NewNotification {
                recipient: &principal,
                kind: NotificationKind::AccessRevoked,
                actor: Some(actor),
                subject: Some(&notifications::snapshot(&entry.entry)),
                access: Some(decode_access(row.access_mask)?),
                access_request_id: None,
                decision: None,
            },
        )
        .await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(())
    }

    async fn create_access_request(
        &self,
        context: &ExecutionContext,
        command: &RequestAccessCommand,
        metadata: &MutationMetadata,
    ) -> Result<AccessRequestView> {
        const OPERATION: &str = "create_access_request";
        let mut request = begin(self, context).await?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            command.entry_id,
            false,
            true,
        )
        .await?
        .ok_or(MetadataRepositoryError::NotFound)?;
        let authorization = entry.authorization(context.authorization());
        let effective = authorization.capabilities().effective_access();
        let already_allowed = command
            .access
            .rights()
            .all(|right| effective.contains(&right.satisfied_by()));
        if already_allowed {
            return Err(MetadataRepositoryError::Conflict);
        }
        let proposed_request_id = AccessRequestId::new();
        let request_id = match claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(proposed_request_id.as_uuid()),
        )
        .await?
        {
            IdempotencyClaim::Replay(Some(id)) => {
                let row = find_access_request_row(&mut request.transaction, id, false)
                    .await?
                    .ok_or(MetadataRepositoryError::Conflict)?;
                let result = access_request_view(row)?;
                request.transaction.commit().await.map_err(map_sql)?;
                return Ok(result);
            }
            IdempotencyClaim::Acquired(Some(id)) => {
                AccessRequestId::from_uuid(id).map_err(internal_data)?
            }
            IdempotencyClaim::Replay(None) | IdempotencyClaim::Acquired(None) => {
                return Err(MetadataRepositoryError::Conflict);
            }
        };
        let actor = context.authorization().actor();
        let row = sqlx::query_as::<_, AccessRequestRow>(
            "INSERT INTO briefcase.access_requests ( \
                    org_id, access_request_id, entry_id, requested_by_type, requested_by_id, \
                    requested_access_mask, reason \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6) \
             RETURNING org_id, access_request_id, entry_id, requested_by_type, requested_by_id, \
                       requested_access_mask, reason, status, granted_access_mask, decided_by_type, \
                       decided_by_id, decided_at, permission_grant_id, created_at, updated_at",
        )
        .bind(request_id.as_uuid())
        .bind(command.entry_id.as_uuid())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(encode_access(command.access))
        .bind(command.reason.as_deref())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(command.entry_id.as_uuid()),
            "access_request.created.v1",
            "access_request",
            &request_id.to_string(),
            json!({
                "entry_id": command.entry_id,
                "requested_access": access_rights(command.access),
            }),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(request_id.as_uuid()),
        )
        .await?;
        // The people who can approve this are the entry owner and every
        // organization owner or admin. The requester never notifies itself.
        let subject = notifications::snapshot(&entry.entry);
        let recipients =
            notifications::decision_recipients(&mut request.transaction, &entry.entry.owner, actor)
                .await?;
        for recipient in &recipients {
            notifications::insert(
                &mut request.transaction,
                &notifications::NewNotification {
                    recipient,
                    kind: NotificationKind::AccessRequested,
                    actor: Some(actor),
                    subject: Some(&subject),
                    access: Some(command.access),
                    access_request_id: Some(request_id),
                    decision: None,
                },
            )
            .await?;
        }
        let result = access_request_view(row)?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(result)
    }

    async fn find_access_request(
        &self,
        context: &ExecutionContext,
        request_id: AccessRequestId,
    ) -> Result<Option<AuthorizableAccessRequest>> {
        let mut request = begin(self, context).await?;
        let row =
            find_access_request_row(&mut request.transaction, request_id.as_uuid(), false).await?;
        let result = if let Some(row) = row {
            let entry_id = EntryId::from_uuid(row.entry_id).map_err(internal_data)?;
            let entry = load_entry(&mut request.transaction, context, entry_id, false, false)
                .await?
                .ok_or(MetadataRepositoryError::NotFound)?;
            Some(AuthorizableAccessRequest {
                request: access_request_view(row)?,
                entry,
            })
        } else {
            None
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(result)
    }

    async fn decide_access_request(
        &self,
        context: &ExecutionContext,
        command: DecideAccessRequestCommand,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AccessRequestView> {
        const OPERATION: &str = "decide_access_request";
        let mut request = begin(self, context).await?;
        let row =
            find_access_request_row(&mut request.transaction, command.request_id.as_uuid(), true)
                .await?
                .ok_or(MetadataRepositoryError::NotFound)?;
        let entry_id = EntryId::from_uuid(row.entry_id).map_err(internal_data)?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, true)
            .await?
            .ok_or(MetadataRepositoryError::NotFound)?;
        require_capability(&entry, context, required_capability)?;
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.request_id.as_uuid()),
        )
        .await?
        {
            let result = access_request_view(row)?;
            request.transaction.commit().await.map_err(map_sql)?;
            return Ok(result);
        }
        if row.status != "pending" {
            return Err(MetadataRepositoryError::Conflict);
        }
        let actor = context.authorization().actor();
        let (status, access, grant_id) = match command.decision {
            AccessDecision::Deny => ("denied", None, None),
            AccessDecision::Approve { access } => {
                let requester = actor_ref(&row.requested_by_type, &row.requested_by_id)?;
                if !current_member(&mut request.transaction, &requester).await? {
                    return Err(MetadataRepositoryError::Conflict);
                }
                let grant_id = GrantId::new();
                sqlx::query(
                    "INSERT INTO briefcase.permission_grants ( \
                            org_id, entry_id, grant_id, principal_type, principal_id, \
                            access_mask, inherits_to_descendants, granted_by_type, granted_by_id \
                     ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, true, $6, $7)",
                )
                .bind(row.entry_id)
                .bind(grant_id.as_uuid())
                .bind(&row.requested_by_type)
                .bind(&row.requested_by_id)
                .bind(encode_access(access))
                .bind(actor_kind(actor.kind()))
                .bind(actor.id().as_str())
                .execute(&mut *request.transaction)
                .await
                .map_err(map_sql)?;
                ("approved", Some(access), Some(grant_id))
            }
        };
        let updated = sqlx::query_as::<_, AccessRequestRow>(
            "UPDATE briefcase.access_requests \
                SET status = $2, granted_access_mask = $3, decided_by_type = $4, \
                    decided_by_id = $5, decided_at = clock_timestamp(), permission_grant_id = $6 \
              WHERE org_id = briefcase.current_org_id() AND access_request_id = $1 \
                AND status = 'pending' \
             RETURNING org_id, access_request_id, entry_id, requested_by_type, requested_by_id, \
                       requested_access_mask, reason, status, granted_access_mask, decided_by_type, \
                       decided_by_id, decided_at, permission_grant_id, created_at, updated_at",
        )
        .bind(command.request_id.as_uuid())
        .bind(status)
        .bind(access.map(encode_access))
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(grant_id.map(GrantId::as_uuid))
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        record_change(
            &mut request.transaction,
            &request.context,
            Some(row.entry_id),
            "access_request.decided.v1",
            "access_request",
            &command.request_id.to_string(),
            json!({"status": status, "grant_id": grant_id}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(command.request_id.as_uuid()),
        )
        .await?;
        let requester = actor_ref(&row.requested_by_type, &row.requested_by_id)?;
        let entry = load_entry(
            &mut request.transaction,
            context,
            EntryId::from_uuid(row.entry_id).map_err(internal_data)?,
            false,
            false,
        )
        .await?;
        notifications::insert(
            &mut request.transaction,
            &notifications::NewNotification {
                recipient: &requester,
                kind: NotificationKind::AccessRequestDecided,
                actor: Some(actor),
                subject: entry
                    .as_ref()
                    .map(|entry| notifications::snapshot(&entry.entry))
                    .as_ref(),
                access,
                access_request_id: Some(command.request_id),
                decision: Some(if access.is_some() {
                    NotificationDecision::Approved
                } else {
                    NotificationDecision::Denied
                }),
            },
        )
        .await?;
        let result = access_request_view(updated)?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(result)
    }

    async fn search(
        &self,
        context: &ExecutionContext,
        query: &SearchQuery,
    ) -> Result<Vec<SearchCandidate>> {
        #[derive(sqlx::FromRow)]
        struct SearchHit {
            entry_id: Uuid,
            score: f32,
            filename_match: bool,
            content_hits: i64,
            snippet: Option<String>,
        }

        let mut request = begin(self, context).await?;
        let authorization = context.authorization();
        let actor = authorization.actor();
        let tags: Vec<&str> = authorization
            .tags()
            .iter()
            .map(crate::domain::actor::TagName::as_str)
            .collect();
        let rows = sqlx::query_as::<_, SearchHit>(
            // Both sides are normalized by the same function, so a term
            // inside a filename matches the name it belongs to.
            "WITH search_query AS ( \
                    SELECT websearch_to_tsquery( \
                               'simple'::regconfig, briefcase.searchable_text($1) \
                           ) AS value, \
                           ARRAY( \
                               SELECT term.lexeme \
                                 FROM unnest(to_tsvector( \
                                          'simple'::regconfig, briefcase.searchable_text($1) \
                                      )) AS term \
                           ) AS lexemes \
             ) \
             SELECT document.entry_id, \
                    (2.0 * ts_rank(document.filename_search, search_query.value) \
                         + ts_rank(document.content_search, search_query.value))::real AS score, \
                    document.filename_search @@ search_query.value AS filename_match, \
                    content.hits AS content_hits, \
                    CASE WHEN document.content_search @@ search_query.value \
                         THEN ts_headline('simple'::regconfig, COALESCE(document.extracted_content, ''), \
                                          search_query.value, 'MaxFragments=2,MaxWords=24,MinWords=8') \
                         ELSE NULL END AS snippet \
               FROM briefcase.search_documents AS document \
               JOIN briefcase.entries AS entry \
                 ON entry.org_id = document.org_id AND entry.entry_id = document.entry_id \
               CROSS JOIN search_query \
     CROSS JOIN LATERAL ( \
                    SELECT COALESCE( \
                               SUM(COALESCE(array_length(word.positions, 1), 0)), \
                               0 \
                           )::bigint AS hits \
                      FROM unnest(document.content_search) AS word \
                     WHERE word.lexeme = ANY(search_query.lexemes) \
                 ) AS content \
          LEFT JOIN briefcase.organization_tags AS tag \
                 ON tag.org_id = entry.org_id AND tag.tag_id = entry.tag_id \
              WHERE document.org_id = briefcase.current_org_id() AND entry.deleted_at IS NULL \
                AND (document.filename_search @@ search_query.value \
                     OR document.content_search @@ search_query.value) \
                AND ( \
                    $2 OR (entry.owner_type = $3 AND entry.owner_id = $4) \
                    OR entry.root_type = 'public' \
                    OR (entry.root_type = 'tag' AND tag.name = ANY($5)) \
                    OR EXISTS ( \
                        SELECT 1 FROM briefcase.entry_closure AS path \
                        JOIN briefcase.permission_grants AS access_grant \
                          ON access_grant.org_id = path.org_id AND access_grant.entry_id = path.ancestor_id \
                       WHERE path.org_id = entry.org_id AND path.descendant_id = entry.entry_id \
                         AND access_grant.principal_type = $3 AND access_grant.principal_id = $4 \
                         AND access_grant.revoked_at IS NULL \
                         AND (path.depth = 0 OR access_grant.inherits_to_descendants) \
                    ) \
                ) \
              ORDER BY filename_match DESC, content_hits DESC, score DESC, document.entry_id \
              LIMIT $6",
        )
        .bind(&query.query)
        .bind(authorization.role().has_administrative_access())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(tags)
        .bind(i64::from(query.limit))
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let entry_id = EntryId::from_uuid(row.entry_id).map_err(internal_data)?;
            let entry = load_entry(&mut request.transaction, context, entry_id, false, false)
                .await?
                .ok_or_else(|| internal("search projection references missing entry"))?;
            results.push(SearchCandidate {
                entry,
                score: f64::from(row.score),
                filename_match: row.filename_match,
                content_hits: u32::try_from(row.content_hits).map_err(internal_data)?,
                snippets: row.snippet.into_iter().collect(),
            });
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(results)
    }

    async fn list_file_versions(
        &self,
        context: &ExecutionContext,
        query: &ListVersionsQuery,
    ) -> Result<Page<FileVersionView>> {
        #[derive(sqlx::FromRow)]
        struct VersionRow {
            version_id: Uuid,
            version_number: i64,
            source: String,
            restored_from_version_id: Option<Uuid>,
            size_bytes: i64,
            created_by_type: String,
            created_by_id: String,
            created_at: OffsetDateTime,
        }

        let cursor = decode_optional::<NumberCursor>(query.page.cursor.as_deref())?;
        let before = cursor.map_or(i64::MAX, |value| value.number);
        let mut request = begin(self, context).await?;
        let rows = sqlx::query_as::<_, VersionRow>(
            "SELECT version_id, version_number, source, restored_from_version_id, size_bytes, \
                    created_by_type, created_by_id, created_at \
               FROM briefcase.entry_versions \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                AND version_number < $2 \
              ORDER BY version_number DESC LIMIT $3",
        )
        .bind(query.entry_id.as_uuid())
        .bind(before)
        .bind(i64::from(query.page.limit) + 1)
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let has_more = rows.len() > usize::from(query.page.limit);
        let mut items = Vec::with_capacity(usize::from(query.page.limit));
        for row in rows.into_iter().take(usize::from(query.page.limit)) {
            let source = match row.source.as_str() {
                "upload" => VersionSource::InitialUpload,
                "restore" => VersionSource::Restore {
                    source_version_id: VersionId::from_uuid(
                        row.restored_from_version_id
                            .ok_or_else(|| internal("restore version has no source"))?,
                    )
                    .map_err(internal_data)?,
                },
                _ => return Err(internal("invalid persisted version source")),
            };
            items.push(FileVersionView {
                id: VersionId::from_uuid(row.version_id).map_err(internal_data)?,
                number: VersionNumber::new(
                    u64::try_from(row.version_number).map_err(internal_data)?,
                )
                .map_err(internal_data)?,
                size: u64::try_from(row.size_bytes).map_err(internal_data)?,
                created_by: actor_ref(&row.created_by_type, &row.created_by_id)?,
                source,
                created_at: row.created_at,
            });
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|version| {
                    i64::try_from(version.number.get())
                        .map_err(internal_data)
                        .and_then(|number| encode_cursor(&NumberCursor { number }))
                })
                .transpose()?
        } else {
            None
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(Page { items, next_cursor })
    }

    async fn list_bin_entries(
        &self,
        context: &ExecutionContext,
        query: &ListBinQuery,
    ) -> Result<Page<AuthorizableEntry>> {
        let cursor = decode_optional::<BinCursor>(query.page.cursor.as_deref())?;
        let mut request = begin(self, context).await?;
        let authorization = context.authorization();
        let actor = authorization.actor();
        let origin = authorization
            .originating_application()
            .map_or("", |application| application.as_str());
        let rows = sqlx::query_as::<_, EntryRow>(concat!(
            "SELECT ",
            entry_columns!(),
            " FROM briefcase.entries AS entry \
              WHERE entry.org_id = briefcase.current_org_id() \
                AND entry.deleted_at IS NOT NULL AND entry.purge_after > clock_timestamp() \
                AND NOT EXISTS ( \
                    SELECT 1 FROM briefcase.entries AS parent \
                     WHERE parent.org_id = entry.org_id AND parent.entry_id = entry.parent_id \
                       AND parent.deletion_batch_id = entry.deletion_batch_id \
                ) \
                AND ($1 = '' OR entry.origin_app_id = $1) \
                AND ( \
                    $2 OR (entry.owner_type = $3 AND entry.owner_id = $4) \
                    OR EXISTS ( \
                        SELECT 1 FROM briefcase.entry_closure AS path \
                        JOIN briefcase.permission_grants AS access_grant \
                          ON access_grant.org_id = path.org_id AND access_grant.entry_id = path.ancestor_id \
                       WHERE path.org_id = entry.org_id AND path.descendant_id = entry.entry_id \
                         AND access_grant.principal_type = $3 AND access_grant.principal_id = $4 \
                         AND (access_grant.access_mask & ~briefcase.access_bit('read')) <> 0 \
                         AND access_grant.revoked_at IS NULL \
                         AND (path.depth = 0 OR access_grant.inherits_to_descendants) \
                    ) \
                ) \
                AND ($5::timestamptz IS NULL OR (entry.deleted_at, entry.entry_id) < ($5, $6)) \
              ORDER BY entry.deleted_at DESC, entry.entry_id DESC LIMIT $7",
        ))
        .bind(origin)
        .bind(authorization.role().has_administrative_access())
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(cursor.map(|value| value.deleted_at))
        .bind(cursor.map_or(Uuid::max(), |value| value.id))
        .bind(i64::from(query.page.limit) + 1)
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let has_more = rows.len() > usize::from(query.page.limit);
        let mut items = Vec::with_capacity(usize::from(query.page.limit));
        for row in rows.into_iter().take(usize::from(query.page.limit)) {
            items.push(build_authorizable(&mut request.transaction, context, row).await?);
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|entry| {
                    encode_cursor(&BinCursor {
                        deleted_at: entry
                            .entry
                            .deleted_at
                            .ok_or_else(|| internal("bin result is not deleted"))?,
                        id: entry.entry.id.as_uuid(),
                    })
                })
                .transpose()?
        } else {
            None
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(Page { items, next_cursor })
    }

    async fn find_bin_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Option<AuthorizableEntry>> {
        let mut request = begin(self, context).await?;
        let is_root = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM briefcase.entries AS entry \
                  WHERE entry.org_id = briefcase.current_org_id() AND entry.entry_id = $1 \
                    AND entry.deleted_at IS NOT NULL AND entry.purge_after > clock_timestamp() \
                    AND NOT EXISTS ( \
                        SELECT 1 FROM briefcase.entries AS parent \
                         WHERE parent.org_id = entry.org_id AND parent.entry_id = entry.parent_id \
                           AND parent.deletion_batch_id = entry.deletion_batch_id \
                    ) \
             )",
        )
        .bind(entry_id.as_uuid())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        let entry = if is_root {
            load_entry(&mut request.transaction, context, entry_id, true, false).await?
        } else {
            None
        };
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entry)
    }

    async fn restore_bin_entry(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        metadata: &MutationMetadata,
        required_capability: Capability,
    ) -> Result<AuthorizableEntry> {
        const OPERATION: &str = "restore_bin_entry";
        let mut request = begin(self, context).await?;
        roots::lock_organization_reconciliation(&mut request.transaction)
            .await
            .map_err(map_sql)?;
        let entry = load_entry(&mut request.transaction, context, entry_id, true, true)
            .await?
            .ok_or(MetadataRepositoryError::NotFound)?;
        if entry.entry.deleted_at.is_none() {
            if let IdempotencyClaim::Replay(_) = claim_idempotency(
                &mut request.transaction,
                &request.context,
                OPERATION,
                metadata,
                Some(entry_id.as_uuid()),
            )
            .await?
            {
                request.transaction.commit().await.map_err(map_sql)?;
                return Ok(entry);
            }
            return Err(MetadataRepositoryError::Conflict);
        }
        require_capability(&entry, context, required_capability)?;
        if entry.system_kind.is_some() {
            return Err(MetadataRepositoryError::Conflict);
        }
        let batch_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT deletion_batch_id FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                AND deleted_at IS NOT NULL AND purge_after > clock_timestamp() \
              FOR UPDATE",
        )
        .bind(entry_id.as_uuid())
        .fetch_optional(&mut *request.transaction)
        .await
        .map_err(map_sql)?
        .ok_or(MetadataRepositoryError::NotFound)?;
        lock_and_require_subtree_capability(
            &mut request.transaction,
            context,
            entry_id,
            Capability::UpdateMetadata,
        )
        .await?;
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(entry_id.as_uuid()),
        )
        .await?
        {
            return Err(MetadataRepositoryError::Conflict);
        }
        let subtree = sqlx::query_as::<_, SubtreeRow>(
            "SELECT child.entry_id, path.depth \
               FROM briefcase.entry_closure AS path \
               JOIN briefcase.entries AS child \
                 ON child.org_id = path.org_id AND child.entry_id = path.descendant_id \
              WHERE path.org_id = briefcase.current_org_id() AND path.ancestor_id = $1 \
                AND child.deletion_batch_id = $2 \
              ORDER BY path.depth, child.entry_id \
              FOR UPDATE OF child",
        )
        .bind(entry_id.as_uuid())
        .bind(batch_id)
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        if subtree.first().is_none_or(|row| row.depth != 0) {
            return Err(MetadataRepositoryError::Conflict);
        }
        let destination = resolve_restore_parent(&mut request.transaction, context, &entry).await?;
        let restored_name = available_restore_name(
            &mut request.transaction,
            destination,
            &entry.entry.name,
            entry_id,
        )
        .await?;
        let actor = context.authorization().actor();
        for child in subtree {
            if child.depth == 0 {
                sqlx::query(
                    "UPDATE briefcase.entries SET parent_id = $2, name = $3, deletion_batch_id = NULL, \
                            deleted_at = NULL, purge_after = NULL, updated_by_type = $4, updated_by_id = $5 \
                      WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                        AND deletion_batch_id = $6",
                )
                .bind(child.entry_id)
                .bind(destination.map(EntryId::as_uuid))
                .bind(restored_name.as_str())
                .bind(actor_kind(actor.kind()))
                .bind(actor.id().as_str())
                .bind(batch_id)
                .execute(&mut *request.transaction)
                .await
                .map_err(map_sql)?;
            } else {
                sqlx::query(
                    "UPDATE briefcase.entries SET deletion_batch_id = NULL, deleted_at = NULL, \
                            purge_after = NULL, updated_by_type = $2, updated_by_id = $3 \
                      WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                        AND deletion_batch_id = $4",
                )
                .bind(child.entry_id)
                .bind(actor_kind(actor.kind()))
                .bind(actor.id().as_str())
                .bind(batch_id)
                .execute(&mut *request.transaction)
                .await
                .map_err(map_sql)?;
            }
        }
        record_change(
            &mut request.transaction,
            &request.context,
            Some(entry_id.as_uuid()),
            "entry.subtree_restored.v1",
            "entry",
            &entry_id.to_string(),
            json!({"deletion_batch_id": batch_id, "parent_id": destination}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            Some(entry_id.as_uuid()),
        )
        .await?;
        let restored = load_entry(&mut request.transaction, context, entry_id, false, false)
            .await?
            .ok_or_else(|| internal("restored entry disappeared"))?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(restored)
    }

    async fn project_member_authorization(
        &self,
        organization_id: &OrganizationId,
        actor: &ActorRef,
        request_id: &str,
    ) -> Result<Option<ProjectedMembership>> {
        #[derive(sqlx::FromRow)]
        struct MembershipRow {
            org_role: String,
            tag_names: Vec<String>,
        }

        // This read runs before any request context exists, so it opens its own
        // tenant transaction and never reconciles anything.
        let context = TenantContext::for_projection(
            organization_id.as_str().to_owned(),
            actor,
            request_id.to_owned(),
        );
        let mut transaction = self.begin(&context).await.map_err(map_sql)?;
        let row = sqlx::query_as::<_, MembershipRow>(
            "SELECT member.org_role, \
                    COALESCE( \
                        array_agg(tag.name ORDER BY tag.name COLLATE \"C\") \
                            FILTER ( \
                                WHERE tag.tag_id IS NOT NULL \
                                  AND tag.lifecycle_status = 'active' \
                            ), \
                        ARRAY[]::text[] \
                    ) AS tag_names \
               FROM briefcase.organization_members AS member \
               LEFT JOIN briefcase.organization_member_tags AS member_tag \
                 ON member_tag.org_id = member.org_id \
                AND member_tag.actor_type = member.actor_type \
                AND member_tag.actor_id = member.actor_id \
               LEFT JOIN briefcase.organization_tags AS tag \
                 ON tag.org_id = member_tag.org_id AND tag.tag_id = member_tag.tag_id \
              WHERE member.org_id = briefcase.current_org_id() \
                AND member.actor_type = $1 AND member.actor_id = $2 \
                AND member.membership_status = 'active' \
              GROUP BY member.org_role",
        )
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_sql)?;
        transaction.commit().await.map_err(map_sql)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let role = match row.org_role.as_str() {
            "owner" => OrganizationRole::Owner,
            "admin" => OrganizationRole::Admin,
            "member" => OrganizationRole::Member,
            _ => return Err(internal("invalid persisted organization role")),
        };
        let tags = row
            .tag_names
            .into_iter()
            .map(|tag| TagName::new(tag).map_err(internal_data))
            .collect::<Result<_>>()?;
        Ok(Some(ProjectedMembership { role, tags }))
    }

    async fn ensure_application_folder(
        &self,
        context: &ExecutionContext,
    ) -> Result<AuthorizableEntry> {
        let application_id = context
            .authorization()
            .originating_application()
            .ok_or_else(|| internal("application folder requires an OBO request"))?
            .as_str()
            .to_owned();
        let actor = context.authorization().actor().clone();
        let mut request = begin(self, context).await?;
        let entry_id = roots::ensure_application_container(
            &mut request.transaction,
            (actor_kind(actor.kind()), actor.id().as_str()),
            &application_id,
        )
        .await
        .map_err(map_sql)?;
        let entry_id = EntryId::from_uuid(entry_id).map_err(internal_data)?;
        let entry = load_entry(&mut request.transaction, context, entry_id, false, false)
            .await?
            .ok_or_else(|| internal("application folder disappeared"))?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(entry)
    }

    async fn list_entry_activity(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
    ) -> Result<Vec<ActivityEvent>> {
        #[derive(sqlx::FromRow)]
        struct ActivityRow {
            actor_type: String,
            actor_id: String,
            origin_app_id: Option<String>,
            action: String,
            occurred_at: OffsetDateTime,
        }

        let mut request = begin(self, context).await?;
        let rows = sqlx::query_as::<_, ActivityRow>(
            "SELECT actor_type, actor_id, origin_app_id, action, occurred_at \
               FROM briefcase.audit_events \
              WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
              ORDER BY occurred_at DESC, audit_id DESC \
              LIMIT $2",
        )
        .bind(entry_id.as_uuid())
        .bind(i64::from(ENTRY_ACTIVITY_HISTORY_SIZE))
        .fetch_all(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        request.transaction.commit().await.map_err(map_sql)?;

        rows.into_iter()
            .map(|row| {
                Ok(ActivityEvent {
                    action: row.action,
                    actor: actor_ref(&row.actor_type, &row.actor_id)?,
                    application_id: row
                        .origin_app_id
                        .filter(|value| !value.is_empty())
                        .map(ApplicationId::new)
                        .transpose()
                        .map_err(internal_data)?,
                    occurred_at: row.occurred_at,
                })
            })
            .collect()
    }

    async fn load_notification_inbox(
        &self,
        context: &ExecutionContext,
    ) -> Result<NotificationInbox> {
        let mut request = begin(self, context).await?;
        let inbox = notifications::load_inbox(&mut request.transaction, context).await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(inbox)
    }

    async fn load_organization_usage(
        &self,
        context: &ExecutionContext,
    ) -> Result<OrganizationUsage> {
        let mut request = begin(self, context).await?;
        let usage = super::quota::read_usage(&mut request.transaction)
            .await
            .map_err(|_| MetadataRepositoryError::Unavailable)?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(usage)
    }

    async fn mark_notifications_read(
        &self,
        context: &ExecutionContext,
        metadata: &MutationMetadata,
    ) -> Result<NotificationInbox> {
        const OPERATION: &str = "mark_notifications_read";
        let mut request = begin(self, context).await?;
        // Marking the inbox read is idempotent by nature, but claiming the key
        // keeps a retried request from writing a second audit record.
        if let IdempotencyClaim::Replay(_) = claim_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            None,
        )
        .await?
        {
            let inbox = notifications::load_inbox(&mut request.transaction, context).await?;
            request.transaction.commit().await.map_err(map_sql)?;
            return Ok(inbox);
        }
        notifications::mark_all_read(&mut request.transaction, context).await?;
        record_change(
            &mut request.transaction,
            &request.context,
            None,
            "notifications.inbox_read.v1",
            "notification_inbox",
            context.authorization().actor().id().as_str(),
            json!({}),
        )
        .await?;
        complete_idempotency(
            &mut request.transaction,
            &request.context,
            OPERATION,
            metadata,
            None,
        )
        .await?;
        let inbox = notifications::load_inbox(&mut request.transaction, context).await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(inbox)
    }

    async fn record_metadata_access(
        &self,
        context: &ExecutionContext,
        entry_ids: &[EntryId],
    ) -> Result<()> {
        let mut request = begin(self, context).await?;
        for entry_id in entry_ids {
            PostgresRepository::insert_audit_event(
                &mut request.transaction,
                &request.context,
                &super::NewAuditEvent {
                    audit_id: Uuid::now_v7(),
                    entry_id: Some(entry_id.as_uuid()),
                    action: "entry.metadata_read.v1".to_owned(),
                    metadata: json!({}),
                },
            )
            .await
            .map_err(map_sql)?;
        }
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(())
    }

    async fn is_current_member(
        &self,
        context: &ExecutionContext,
        principal: &ActorRef,
    ) -> Result<bool> {
        let mut request = begin(self, context).await?;
        let exists = current_member(&mut request.transaction, principal).await?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(exists)
    }

    async fn grant_exists(
        &self,
        context: &ExecutionContext,
        entry_id: EntryId,
        grant_id: GrantId,
    ) -> Result<bool> {
        let mut request = begin(self, context).await?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM briefcase.permission_grants \
                  WHERE org_id = briefcase.current_org_id() AND entry_id = $1 \
                    AND grant_id = $2 AND revoked_at IS NULL \
             )",
        )
        .bind(entry_id.as_uuid())
        .bind(grant_id.as_uuid())
        .fetch_one(&mut *request.transaction)
        .await
        .map_err(map_sql)?;
        request.transaction.commit().await.map_err(map_sql)?;
        Ok(exists)
    }
}

async fn find_grant(
    transaction: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    lock: bool,
) -> Result<Option<PermissionGrantRow>> {
    if lock {
        sqlx::query_as::<_, PermissionGrantRow>(
            "SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                    inherits_to_descendants, granted_by_type, granted_by_id, revoked_at, \
                    revoked_by_type, revoked_by_id, created_at \
               FROM briefcase.permission_grants \
              WHERE org_id = briefcase.current_org_id() AND grant_id = $1 FOR UPDATE",
        )
        .bind(grant_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql)
    } else {
        sqlx::query_as::<_, PermissionGrantRow>(
            "SELECT org_id, entry_id, grant_id, principal_type, principal_id, access_mask, \
                    inherits_to_descendants, granted_by_type, granted_by_id, revoked_at, \
                    revoked_by_type, revoked_by_id, created_at \
               FROM briefcase.permission_grants \
              WHERE org_id = briefcase.current_org_id() AND grant_id = $1",
        )
        .bind(grant_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql)
    }
}

async fn find_access_request_row(
    transaction: &mut Transaction<'_, Postgres>,
    request_id: Uuid,
    lock: bool,
) -> Result<Option<AccessRequestRow>> {
    if lock {
        sqlx::query_as::<_, AccessRequestRow>(
            "SELECT org_id, access_request_id, entry_id, requested_by_type, requested_by_id, \
                    requested_access_mask, reason, status, granted_access_mask, decided_by_type, \
                    decided_by_id, decided_at, permission_grant_id, created_at, updated_at \
               FROM briefcase.access_requests \
              WHERE org_id = briefcase.current_org_id() AND access_request_id = $1 FOR UPDATE",
        )
        .bind(request_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql)
    } else {
        sqlx::query_as::<_, AccessRequestRow>(
            "SELECT org_id, access_request_id, entry_id, requested_by_type, requested_by_id, \
                    requested_access_mask, reason, status, granted_access_mask, decided_by_type, \
                    decided_by_id, decided_at, permission_grant_id, created_at, updated_at \
               FROM briefcase.access_requests \
              WHERE org_id = briefcase.current_org_id() AND access_request_id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql)
    }
}

/// Renders an access set as the right names used in audit and event payloads.
fn access_rights(access: GrantedAccess) -> Vec<&'static str> {
    access
        .rights()
        .map(|right| match right {
            AccessRight::Read => "read",
            AccessRight::Write => "write",
            AccessRight::Update => "update",
            AccessRight::Delete => "delete",
        })
        .collect()
}

fn access_request_view(row: AccessRequestRow) -> Result<AccessRequestView> {
    let status = match row.status.as_str() {
        "pending" => AccessRequestStatus::Pending,
        "approved" => AccessRequestStatus::Approved,
        "denied" => AccessRequestStatus::Denied,
        _ => return Err(internal("invalid persisted access-request status")),
    };
    let decided_by = match (row.decided_by_type.as_deref(), row.decided_by_id.as_deref()) {
        (Some(kind), Some(id)) => Some(actor_ref(kind, id)?),
        (None, None) => None,
        _ => return Err(internal("incomplete access-request decision actor")),
    };
    Ok(AccessRequestView {
        id: AccessRequestId::from_uuid(row.access_request_id).map_err(internal_data)?,
        entry_id: EntryId::from_uuid(row.entry_id).map_err(internal_data)?,
        requested_by: actor_ref(&row.requested_by_type, &row.requested_by_id)?,
        requested_access: decode_access(row.requested_access_mask)?,
        reason: row.reason,
        status,
        granted_access: row.granted_access_mask.map(decode_access).transpose()?,
        decided_by,
        decided_at: row.decided_at,
        permission_grant_id: row
            .permission_grant_id
            .map(GrantId::from_uuid)
            .transpose()
            .map_err(internal_data)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

fn require_capability(
    entry: &AuthorizableEntry,
    context: &ExecutionContext,
    capability: Capability,
) -> Result<()> {
    if entry
        .authorization(context.authorization())
        .allows(capability)
    {
        Ok(())
    } else {
        Err(MetadataRepositoryError::Conflict)
    }
}

async fn lock_and_require_subtree_capability(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    root_id: EntryId,
    capability: Capability,
) -> Result<()> {
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT entry.entry_id \
           FROM briefcase.entry_closure AS path \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = path.org_id AND entry.entry_id = path.descendant_id \
          WHERE path.org_id = briefcase.current_org_id() AND path.ancestor_id = $1 \
          ORDER BY path.depth, entry.entry_id FOR UPDATE OF entry",
    )
    .bind(root_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if locked.is_empty() {
        return Err(MetadataRepositoryError::NotFound);
    }

    if !matches!(capability, Capability::Delete | Capability::UpdateMetadata) {
        return Err(internal("unsupported recursive capability"));
    }
    let authorization = context.authorization();
    let actor = authorization.actor();
    let origin = if capability == Capability::Delete {
        authorization
            .originating_application()
            .map(crate::domain::actor::ApplicationId::as_str)
    } else {
        None
    };
    let allowed = sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE(bool_and( \
                    entry.system_kind IS NULL \
                    AND ($5::text IS NULL OR entry.origin_app_id = $5) \
                    AND ( \
                        $2 OR (entry.owner_type = $3 AND entry.owner_id = $4) \
                        OR EXISTS ( \
                            SELECT 1 FROM briefcase.entry_closure AS grant_path \
                            JOIN briefcase.permission_grants AS access_grant \
                              ON access_grant.org_id = grant_path.org_id \
                             AND access_grant.entry_id = grant_path.ancestor_id \
                           WHERE grant_path.org_id = entry.org_id \
                             AND grant_path.descendant_id = entry.entry_id \
                             AND access_grant.principal_type = $3 AND access_grant.principal_id = $4 \
                             AND (access_grant.access_mask & ~briefcase.access_bit('read')) <> 0 \
                         AND access_grant.revoked_at IS NULL \
                             AND (grant_path.depth = 0 OR access_grant.inherits_to_descendants) \
                        ) \
                    ) \
                ), false) \
           FROM briefcase.entry_closure AS subtree \
           JOIN briefcase.entries AS entry \
             ON entry.org_id = subtree.org_id AND entry.entry_id = subtree.descendant_id \
          WHERE subtree.org_id = briefcase.current_org_id() AND subtree.ancestor_id = $1",
    )
    .bind(root_id.as_uuid())
    .bind(authorization.role().has_administrative_access())
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(origin)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if allowed {
        Ok(())
    } else {
        Err(MetadataRepositoryError::Conflict)
    }
}

async fn resolve_restore_parent(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
    entry: &AuthorizableEntry,
) -> Result<Option<EntryId>> {
    if let Some(parent_id) = entry.entry.parent_id
        && let Some(parent) = load_entry(transaction, context, parent_id, false, true).await?
        && parent.entry.kind == EntryKind::Folder
        && parent.entry.boundary == entry.entry.boundary
        && parent
            .authorization(context.authorization())
            .allows(Capability::CreateChild)
    {
        return Ok(Some(parent_id));
    }
    if entry.entry.parent_id.is_none() {
        return Ok(None);
    }

    let actor = context.authorization().actor();
    let tag_name = entry
        .entry
        .boundary
        .tag()
        .map(crate::domain::actor::TagName::as_str);
    let fallback = sqlx::query_scalar::<_, Uuid>(
        "SELECT entry.entry_id \
           FROM briefcase.entries AS entry \
      LEFT JOIN briefcase.organization_tags AS tag \
             ON tag.org_id = entry.org_id AND tag.tag_id = entry.tag_id \
          WHERE entry.org_id = briefcase.current_org_id() AND entry.deleted_at IS NULL \
            AND ( \
                ($1 = 'public' AND entry.system_kind = 'public_root') \
                OR ($1 = 'tag' AND entry.system_kind = 'tag_root' AND tag.name = $2) \
                OR ($1 = 'private' AND ( \
                    (entry.system_kind = 'actor_root' AND entry.owner_type = $3 AND entry.owner_id = $4) \
                    OR entry.system_kind = 'private_root' \
                )) \
            ) \
          ORDER BY CASE WHEN entry.system_kind = 'actor_root' THEN 0 ELSE 1 END \
          LIMIT 1 FOR UPDATE OF entry",
    )
    .bind(match entry.entry.boundary.root_type() {
        crate::domain::entry::RootType::Public => "public",
        crate::domain::entry::RootType::Private => "private",
        crate::domain::entry::RootType::Tag => "tag",
    })
    .bind(tag_name)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?
    .ok_or(MetadataRepositoryError::Conflict)?;
    let fallback_id = EntryId::from_uuid(fallback).map_err(internal_data)?;
    let fallback_entry = load_entry(transaction, context, fallback_id, false, false)
        .await?
        .ok_or_else(|| internal("restore fallback disappeared"))?;
    require_capability(&fallback_entry, context, Capability::CreateChild)?;
    Ok(Some(fallback_id))
}

async fn available_restore_name(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: Option<EntryId>,
    original: &crate::domain::entry::EntryName,
    entry_id: EntryId,
) -> Result<crate::domain::entry::EntryName> {
    let occupied = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 FROM briefcase.entries \
              WHERE org_id = briefcase.current_org_id() \
                AND parent_id IS NOT DISTINCT FROM $1 AND name = $2 \
                AND deleted_at IS NULL AND entry_id <> $3 \
         )",
    )
    .bind(parent_id.map(EntryId::as_uuid))
    .bind(original.as_str())
    .bind(entry_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if !occupied {
        return Ok(original.clone());
    }
    let identifier = entry_id.to_string();
    let suffix = format!(" (restored {})", &identifier[..8]);
    let maximum_prefix_bytes = 255_usize.saturating_sub(suffix.len());
    let mut prefix = original.as_str();
    while prefix.len() > maximum_prefix_bytes {
        let Some((index, _)) = prefix.char_indices().next_back() else {
            break;
        };
        prefix = &prefix[..index];
    }
    crate::domain::entry::EntryName::new(format!("{prefix}{suffix}")).map_err(internal_data)
}

fn encode_cursor<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_vec(value)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(internal_data)
}

fn decode_optional<T>(value: Option<&str>) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    value
        .map(|value| {
            URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| MetadataRepositoryError::InvalidCursor)
                .and_then(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|_| MetadataRepositoryError::InvalidCursor)
                })
        })
        .transpose()
}

fn internal_data(error: impl std::error::Error + Send + Sync + 'static) -> MetadataRepositoryError {
    MetadataRepositoryError::Internal(anyhow::Error::new(error))
}
