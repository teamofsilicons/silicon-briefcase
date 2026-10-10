use anyhow::anyhow;
use serde_json::{Value, json};
use sqlx::{Postgres, QueryBuilder, Transaction};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    application::{
        context::ExecutionContext,
        service::{
            AuthorizableEntry, EntryView, MetadataRepositoryError, MutationMetadata,
            ResolvedGrantScope, ResolvedPermissionGrant,
        },
    },
    domain::{
        actor::{ActorId, ActorKind, ActorRef, ApplicationId, OrganizationId, TagName},
        entry::{EntryBoundary, EntryKind, EntryName, EntryPath, SystemEntryKind},
        ids::{EntryId, GrantId, VersionId},
        permission::{GrantedAccess, PermissionGrant, PermissionGrantParts, PermissionInheritance},
    },
};

use super::super::{
    EntryRow, NewAuditEvent, NewOutboxEvent, PermissionGrantRow, PostgresRepository, TenantContext,
};

pub(in crate::infrastructure::postgres) type Result<T> =
    std::result::Result<T, MetadataRepositoryError>;

/// Trusted actor expression used by the shared owned-ancestor SQL predicate.
#[derive(Clone, Copy)]
pub(in crate::infrastructure::postgres) enum OwnedAncestorPrincipal<'a> {
    /// The caller's creator identities: itself and the Silicons it looks after.
    Creators(&'a [String]),
    /// Correlate with the `reader` account alias used by `accessible-to`: the
    /// reader and the Silicons it looks after.
    Reader,
}

pub(in crate::infrastructure::postgres) struct RequestTransaction<'pool> {
    pub(in crate::infrastructure::postgres) context: TenantContext,
    pub(in crate::infrastructure::postgres) transaction: Transaction<'pool, Postgres>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::infrastructure::postgres) enum IdempotencyClaim {
    Acquired(Option<Uuid>),
    Replay(Option<Uuid>),
}

#[derive(sqlx::FromRow)]
struct IdempotencyState {
    request_hash: Vec<u8>,
    status: String,
    resource_id: Option<Uuid>,
    locked_until: OffsetDateTime,
}

pub(in crate::infrastructure::postgres) async fn begin<'pool>(
    repository: &'pool PostgresRepository,
    execution: &ExecutionContext,
) -> Result<RequestTransaction<'pool>> {
    let context = TenantContext::from_execution(execution);
    let mut transaction = repository.begin(&context).await.map_err(map_sql)?;
    super::super::drives::ensure_participation(&mut transaction, execution.authorization()).await?;
    // Accounts a share names become participants of the drive so the grant
    // can name them. They never execute this request; the caller's row
    // security and audit context stay in force.
    for recipient in execution.share_recipients() {
        super::super::drives::add_share_recipient(&mut transaction, recipient).await?;
    }
    Ok(RequestTransaction {
        context,
        transaction,
    })
}

pub(in crate::infrastructure::postgres) async fn load_entry(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    entry_id: EntryId,
    include_deleted: bool,
    lock: bool,
) -> Result<Option<AuthorizableEntry>> {
    let row = if lock {
        PostgresRepository::lock_entry(transaction, entry_id.as_uuid())
            .await
            .map_err(map_sql)?
    } else {
        PostgresRepository::find_entry(transaction, entry_id.as_uuid())
            .await
            .map_err(map_sql)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    if !include_deleted && row.deleted_at.is_some() {
        return Ok(None);
    }
    build_authorizable(transaction, execution, row)
        .await
        .map(Some)
}

pub(in crate::infrastructure::postgres) async fn build_authorizable(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    row: EntryRow,
) -> Result<AuthorizableEntry> {
    let tag_name = match row.tag_id.as_deref() {
        Some(tag_id) => sqlx::query_scalar::<_, String>(
            "SELECT name FROM briefcase.organization_tags \
              WHERE org_id = briefcase.current_org_id() AND tag_id = $1",
        )
        .bind(tag_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql)?,
        None => None,
    };
    let grants = load_relevant_grants(transaction, execution, row.entry_id).await?;
    let owns_ancestor = owns_containing_folder(transaction, execution, row.entry_id).await?;
    let required_for_traversal = if row.deleted_at.is_some() {
        false
    } else {
        has_visible_descendant(transaction, execution, row.entry_id).await?
    };
    Ok(AuthorizableEntry {
        entry: entry_view(&row, tag_name.as_deref(), execution)?,
        system_kind: system_kind(row.system_kind.as_deref())?,
        grants,
        owns_ancestor,
        required_for_traversal,
        database_filter_matches: Vec::new(),
    })
}

/// Reports whether the caller (or a Silicon it looks after) created a folder
/// the entry sits inside.
///
/// System folders are excluded: a drive's root and its apps folders belong to
/// the drive, whose rights come from the drive itself, and the legacy
/// organization containers name whoever first materialized them.
async fn owns_containing_folder(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    entry_id: Uuid,
) -> Result<bool> {
    let creators = execution.authorization().creator_ids();
    let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
    push_owned_ancestor_access(&mut builder, OwnedAncestorPrincipal::Creators(&creators));
    // The shared predicate is correlated on `entry`; provide the target row
    // exactly as the listing/search queries do.
    builder.push(" FROM briefcase.entries AS entry WHERE entry.org_id = briefcase.current_org_id() AND entry.entry_id = ");
    builder.push_bind(entry_id);
    builder
        .build_query_scalar::<bool>()
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_sql)
}

/// Appends the SQL equivalent of the domain's owned-ancestor read rule.
///
/// Both search and `accessible-to` filtering must use this predicate before
/// their result limits are applied. Otherwise a peer-created descendant that
/// policy permits its containing-folder owner to read is discarded before
/// domain authorization gets a chance to confirm it.
pub(in crate::infrastructure::postgres) fn push_owned_ancestor_access(
    builder: &mut QueryBuilder<Postgres>,
    principal: OwnedAncestorPrincipal<'_>,
) {
    builder.push(
        "EXISTS ( \
             SELECT 1 \
               FROM briefcase.entry_closure AS owned_path \
               JOIN briefcase.entries AS owned_ancestor \
                 ON owned_ancestor.org_id = owned_path.org_id \
                AND owned_ancestor.entry_id = owned_path.ancestor_id \
              WHERE owned_path.org_id = entry.org_id \
                AND owned_path.descendant_id = entry.entry_id \
                AND owned_path.depth > 0 \
                AND owned_ancestor.entry_type = 'folder' \
                AND owned_ancestor.system_kind IS NULL \
                AND ",
    );
    match principal {
        OwnedAncestorPrincipal::Creators(ids) => {
            builder.push("owned_ancestor.owner_id = ANY(");
            builder.push_bind(ids.to_vec());
            builder.push(")");
        }
        OwnedAncestorPrincipal::Reader => {
            builder.push(
                "(owned_ancestor.owner_id = reader.account_uuid \
                  OR EXISTS (SELECT 1 FROM briefcase.accounts AS ancestor_creator \
                              WHERE ancestor_creator.account_uuid = owned_ancestor.owner_id \
                                AND ancestor_creator.kind = 'silicon' \
                                AND ancestor_creator.custodian_uuid = reader.account_uuid))",
            );
        }
    }
    builder.push(")");
}

async fn load_relevant_grants(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    entry_id: Uuid,
) -> Result<Vec<ResolvedPermissionGrant>> {
    #[derive(sqlx::FromRow)]
    struct ResolvedGrantRow {
        org_id: String,
        entry_id: Uuid,
        grant_id: Uuid,
        principal_type: String,
        principal_id: String,
        access_mask: i16,
        inherits_to_descendants: bool,
        granted_by_type: String,
        granted_by_id: String,
        revoked_at: Option<OffsetDateTime>,
        revoked_by_type: Option<String>,
        revoked_by_id: Option<String>,
        created_at: OffsetDateTime,
        expires_at: Option<OffsetDateTime>,
        depth: i32,
    }

    let actor = execution.authorization().actor();
    let rows = sqlx::query_as::<_, ResolvedGrantRow>(
        "SELECT access_grant.org_id, access_grant.entry_id, access_grant.grant_id, access_grant.principal_type, \
                access_grant.principal_id, access_grant.access_mask, access_grant.inherits_to_descendants, \
                access_grant.granted_by_type, access_grant.granted_by_id, access_grant.revoked_at, \
                access_grant.revoked_by_type, access_grant.revoked_by_id, access_grant.created_at, \
                access_grant.expires_at, path.depth \
           FROM briefcase.entry_closure AS path \
           JOIN briefcase.effective_permission_grants AS access_grant \
             ON access_grant.org_id = path.org_id AND access_grant.entry_id = path.ancestor_id \
          WHERE path.org_id = briefcase.current_org_id() \
            AND path.descendant_id = $1 \
            AND access_grant.principal_type = $2 AND access_grant.principal_id = $3 \
            AND access_grant.revoked_at IS NULL \
            AND (path.depth = 0 OR access_grant.inherits_to_descendants) \
          ORDER BY path.depth, access_grant.grant_id",
    )
    .bind(entry_id)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql)?;

    rows.into_iter()
        .map(|row| {
            let grant_row = PermissionGrantRow {
                org_id: row.org_id,
                entry_id: row.entry_id,
                grant_id: row.grant_id,
                principal_type: row.principal_type,
                principal_id: row.principal_id,
                access_mask: row.access_mask,
                inherits_to_descendants: row.inherits_to_descendants,
                granted_by_type: row.granted_by_type,
                granted_by_id: row.granted_by_id,
                revoked_at: row.revoked_at,
                revoked_by_type: row.revoked_by_type,
                revoked_by_id: row.revoked_by_id,
                created_at: row.created_at,
                expires_at: row.expires_at,
            };
            Ok(ResolvedPermissionGrant {
                grant: permission_grant(&grant_row, execution)?,
                scope: if row.depth == 0 {
                    ResolvedGrantScope::Direct
                } else {
                    ResolvedGrantScope::Inherited
                },
            })
        })
        .collect()
}

async fn has_visible_descendant(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
    entry_id: Uuid,
) -> Result<bool> {
    let authorization = execution.authorization();
    if authorization.has_owner_rights() {
        return Ok(false);
    }
    let actor = authorization.actor();
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 \
               FROM briefcase.entry_closure AS subtree \
               JOIN briefcase.entries AS child \
                 ON child.org_id = subtree.org_id AND child.entry_id = subtree.descendant_id \
              WHERE subtree.org_id = briefcase.current_org_id() \
                AND subtree.ancestor_id = $1 AND subtree.depth > 0 \
                AND child.deleted_at IS NULL \
                AND ( \
                    (child.system_kind IS NULL AND child.owner_id = ANY($4)) \
                    OR EXISTS ( \
                        SELECT 1 \
                          FROM briefcase.entry_closure AS grant_path \
                          JOIN briefcase.effective_permission_grants AS access_grant \
                            ON access_grant.org_id = grant_path.org_id \
                           AND access_grant.entry_id = grant_path.ancestor_id \
                         WHERE grant_path.org_id = child.org_id \
                           AND grant_path.descendant_id = child.entry_id \
                           AND access_grant.principal_type = $2 AND access_grant.principal_id = $3 \
                           AND access_grant.revoked_at IS NULL \
                           AND (grant_path.depth = 0 OR access_grant.inherits_to_descendants) \
                    ) \
                ) \
         )",
    )
    .bind(entry_id)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(authorization.creator_ids())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)
}

pub(in crate::infrastructure::postgres) fn entry_view(
    row: &EntryRow,
    tag_name: Option<&str>,
    execution: &ExecutionContext,
) -> Result<EntryView> {
    let organization_id = public_organization_id(&row.org_id, execution)?;
    let boundary = match row.root_type.as_str() {
        "public" => EntryBoundary::Public,
        "private" => EntryBoundary::Private,
        "tag" => EntryBoundary::Tag {
            tag: TagName::new(tag_name.ok_or_else(|| internal("missing entry tag projection"))?)
                .map_err(invalid_data)?,
        },
        _ => return Err(internal("invalid entry root type")),
    };
    let path = if row.system_kind.as_deref() == Some("drive_root") {
        EntryPath::drive_root()
    } else {
        EntryPath::new(&row.path).map_err(invalid_data)?
    };
    Ok(EntryView {
        id: EntryId::from_uuid(row.entry_id).map_err(invalid_data)?,
        organization_id,
        kind: entry_kind(&row.entry_type)?,
        name: EntryName::new(&row.name).map_err(invalid_data)?,
        path,
        parent_id: row
            .parent_id
            .map(EntryId::from_uuid)
            .transpose()
            .map_err(invalid_data)?,
        boundary,
        owner: actor_ref(&row.owner_type, &row.owner_id)?,
        reserved: matches!(
            row.system_kind.as_deref(),
            Some("public_root" | "private_root" | "tag_root")
        ),
        origin_application_id: row
            .origin_app_id
            .as_deref()
            .map(ApplicationId::new)
            .transpose()
            .map_err(invalid_data)?,
        content_type: row.content_type.clone(),
        size: row
            .size_bytes
            .map(u64::try_from)
            .transpose()
            .map_err(invalid_data)?,
        current_version_id: row
            .current_version_id
            .map(VersionId::from_uuid)
            .transpose()
            .map_err(invalid_data)?,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted_at: row.deleted_at,
        self_destruct_at: row.self_destruct_at,
    })
}

pub(in crate::infrastructure::postgres) fn permission_grant(
    row: &PermissionGrantRow,
    execution: &ExecutionContext,
) -> Result<PermissionGrant> {
    let organization_id = public_organization_id(&row.org_id, execution)?;
    Ok(PermissionGrant::from_parts(PermissionGrantParts {
        id: GrantId::from_uuid(row.grant_id).map_err(invalid_data)?,
        organization_id,
        entry_id: EntryId::from_uuid(row.entry_id).map_err(invalid_data)?,
        principal: actor_ref(&row.principal_type, &row.principal_id)?,
        access: decode_access(row.access_mask)?,
        inheritance: PermissionInheritance::from_inherit_flag(row.inherits_to_descendants),
        granted_by: actor_ref(&row.granted_by_type, &row.granted_by_id)?,
        created_at: row.created_at,
    })
    .with_expiry(row.expires_at))
}

/// Converts the database tenant key back to the authenticated tenant.
///
/// A row outside the authenticated storage tenant must be rejected even if a
/// query or RLS invariant regresses.
fn public_organization_id(
    stored_organization_id: &str,
    execution: &ExecutionContext,
) -> Result<OrganizationId> {
    let tenant = TenantContext::from_execution(execution);
    if stored_organization_id != tenant.org_id() {
        return Err(internal(
            "persisted row belongs to a different storage tenant",
        ));
    }
    Ok(execution.authorization().organization_id().clone())
}

pub(in crate::infrastructure::postgres) fn actor_ref(kind: &str, id: &str) -> Result<ActorRef> {
    let kind = match kind {
        "carbon" => ActorKind::Carbon,
        "silicon" => ActorKind::Silicon,
        _ => return Err(internal("invalid persisted actor kind")),
    };
    Ok(ActorRef::new(kind, ActorId::new(id).map_err(invalid_data)?))
}

pub(in crate::infrastructure::postgres) const fn actor_kind(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "carbon",
        ActorKind::Silicon => "silicon",
    }
}

pub(in crate::infrastructure::postgres) const fn encode_access(access: GrantedAccess) -> i16 {
    access.bits() as i16
}

pub(in crate::infrastructure::postgres) fn decode_access(value: i16) -> Result<GrantedAccess> {
    u8::try_from(value)
        .ok()
        .and_then(|bits| GrantedAccess::from_bits(bits).ok())
        .ok_or_else(|| internal("invalid persisted access mask"))
}

pub(in crate::infrastructure::postgres) fn entry_kind(value: &str) -> Result<EntryKind> {
    match value {
        "file" => Ok(EntryKind::File),
        "folder" => Ok(EntryKind::Folder),
        _ => Err(internal("invalid persisted entry kind")),
    }
}

pub(in crate::infrastructure::postgres) fn system_kind(
    value: Option<&str>,
) -> Result<Option<SystemEntryKind>> {
    match value {
        None => Ok(None),
        Some("drive_root") => Ok(Some(SystemEntryKind::DriveRoot)),
        Some("apps_root") => Ok(Some(SystemEntryKind::ApplicationContainer)),
        Some("app_root") => Ok(Some(SystemEntryKind::ApplicationFolder)),
        Some("public_root" | "app_public") => Ok(Some(SystemEntryKind::PublicContainer)),
        Some("private_root" | "app_private") => Ok(Some(SystemEntryKind::PrivateContainer)),
        Some("tag_root") => Ok(Some(SystemEntryKind::TagRoot)),
        Some("actor_root" | "app_container" | "app_actor") => {
            Ok(Some(SystemEntryKind::PrivateActorFolder))
        }
        Some(_) => Err(internal("invalid persisted system entry kind")),
    }
}

pub(in crate::infrastructure::postgres) fn boundary_columns(
    boundary: &EntryBoundary,
) -> (&'static str, Option<&str>) {
    match boundary {
        EntryBoundary::Public => ("public", None),
        EntryBoundary::Private => ("private", None),
        EntryBoundary::Tag { tag } => ("tag", Some(tag.as_str())),
    }
}

pub(in crate::infrastructure::postgres) async fn resolve_tag_id(
    transaction: &mut Transaction<'_, Postgres>,
    tag_name: Option<&str>,
) -> Result<Option<String>> {
    let Some(tag_name) = tag_name else {
        return Ok(None);
    };
    sqlx::query_scalar::<_, String>(
        "SELECT tag_id FROM briefcase.organization_tags \
          WHERE org_id = briefcase.current_org_id() \
            AND name = $1 AND lifecycle_status = 'active'",
    )
    .bind(tag_name)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?
    .map(Some)
    .ok_or(MetadataRepositoryError::Conflict)
}

pub(in crate::infrastructure::postgres) async fn current_member(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &ActorRef,
) -> Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 FROM briefcase.organization_members \
              WHERE org_id = briefcase.current_org_id() \
                AND actor_type = $1 AND actor_id = $2 \
                AND membership_status = 'active' \
         )",
    )
    .bind(actor_kind(principal.kind()))
    .bind(principal.id().as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)
}

pub(in crate::infrastructure::postgres) async fn claim_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    context: &TenantContext,
    operation: &'static str,
    metadata: &MutationMetadata,
    resource_id: Option<Uuid>,
) -> Result<IdempotencyClaim> {
    let Some(key) = metadata.idempotency_key.as_ref() else {
        return Ok(IdempotencyClaim::Acquired(resource_id));
    };
    let inserted = sqlx::query(
        "INSERT INTO briefcase.idempotency_records ( \
                org_id, actor_type, actor_id, origin_app_id, operation, idempotency_key, \
                request_hash, resource_id, locked_until, expires_at \
         ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, \
                   clock_timestamp() + interval '5 minutes', \
                   clock_timestamp() + interval '24 hours') \
         ON CONFLICT DO NOTHING",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(operation)
    .bind(key.as_str())
    .bind(metadata.request_fingerprint.as_slice())
    .bind(resource_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if inserted.rows_affected() == 1 {
        return Ok(IdempotencyClaim::Acquired(resource_id));
    }

    let state = sqlx::query_as::<_, IdempotencyState>(
        "SELECT request_hash, status, resource_id, locked_until \
           FROM briefcase.idempotency_records \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
            AND operation = $4 AND idempotency_key = $5 \
          FOR UPDATE",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(operation)
    .bind(key.as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if state.request_hash.as_slice() != metadata.request_fingerprint {
        return Err(MetadataRepositoryError::Conflict);
    }
    if state.status == "completed" {
        return Ok(IdempotencyClaim::Replay(state.resource_id));
    }
    if state.status != "in_progress" {
        return Err(internal("invalid idempotency state"));
    }
    if state.locked_until > OffsetDateTime::now_utc() {
        return Err(MetadataRepositoryError::Conflict);
    }
    let persisted_resource_id = sqlx::query_scalar::<_, Option<Uuid>>(
        "UPDATE briefcase.idempotency_records \
            SET locked_until = clock_timestamp() + interval '5 minutes', \
                expires_at = clock_timestamp() + interval '24 hours', \
                resource_id = COALESCE(resource_id, $6) \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
            AND operation = $4 AND idempotency_key = $5 \
         RETURNING resource_id",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(operation)
    .bind(key.as_str())
    .bind(resource_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;
    Ok(IdempotencyClaim::Acquired(persisted_resource_id))
}

pub(in crate::infrastructure::postgres) async fn complete_idempotency(
    transaction: &mut Transaction<'_, Postgres>,
    context: &TenantContext,
    operation: &'static str,
    metadata: &MutationMetadata,
    resource_id: Option<Uuid>,
) -> Result<()> {
    let Some(key) = metadata.idempotency_key.as_ref() else {
        return Ok(());
    };
    let result = sqlx::query(
        "UPDATE briefcase.idempotency_records \
            SET status = 'completed', response_status = 200, response_headers = '{}'::jsonb, \
                response_body = jsonb_build_object('resource_id', $6::text), \
                resource_id = COALESCE($6, resource_id), locked_until = clock_timestamp() \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2 AND origin_app_id = $3 \
            AND operation = $4 AND idempotency_key = $5 \
            AND request_hash = $7 AND status = 'in_progress'",
    )
    .bind(context.actor_type())
    .bind(context.actor_id())
    .bind(context.origin_app_id().unwrap_or_default())
    .bind(operation)
    .bind(key.as_str())
    .bind(resource_id)
    .bind(metadata.request_fingerprint.as_slice())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(MetadataRepositoryError::Conflict)
    }
}

pub(in crate::infrastructure::postgres) async fn record_change(
    transaction: &mut Transaction<'_, Postgres>,
    context: &TenantContext,
    entry_id: Option<Uuid>,
    action: &'static str,
    aggregate_type: &'static str,
    aggregate_id: &str,
    payload: Value,
) -> Result<()> {
    PostgresRepository::insert_audit_event(
        transaction,
        context,
        &NewAuditEvent {
            audit_id: Uuid::now_v7(),
            entry_id,
            action: action.to_owned(),
            metadata: payload.clone(),
        },
    )
    .await
    .map_err(map_sql)?;
    PostgresRepository::insert_outbox_event(
        transaction,
        context,
        &NewOutboxEvent {
            event_id: Uuid::now_v7(),
            topic: "briefcase.domain-events.v1".to_owned(),
            aggregate_type: aggregate_type.to_owned(),
            aggregate_id: aggregate_id.to_owned(),
            aggregate_version: None,
            payload: json!({
                "schema_version": 1,
                "type": action,
                "aggregate_type": aggregate_type,
                "aggregate_id": aggregate_id,
                "data": payload,
            }),
            available_at: OffsetDateTime::now_utc(),
        },
    )
    .await
    .map_err(map_sql)?;
    Ok(())
}

/// Formats an instant for audit and event payloads.
pub(in crate::infrastructure::postgres) fn rfc3339(instant: OffsetDateTime) -> String {
    instant
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub(in crate::infrastructure::postgres) fn retention_deadline() -> OffsetDateTime {
    OffsetDateTime::now_utc() + Duration::days(45)
}

pub(in crate::infrastructure::postgres) fn map_sql(error: sqlx::Error) -> MetadataRepositoryError {
    match &error {
        sqlx::Error::RowNotFound => MetadataRepositoryError::NotFound,
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => {
            MetadataRepositoryError::Unavailable
        }
        sqlx::Error::Database(database)
            if matches!(
                database.code().as_deref(),
                Some("23503" | "23505" | "23514" | "40001" | "40P01")
            ) =>
        {
            MetadataRepositoryError::Conflict
        }
        _ => MetadataRepositoryError::Internal(anyhow!(error)),
    }
}

pub(in crate::infrastructure::postgres) fn internal(
    message: &'static str,
) -> MetadataRepositoryError {
    MetadataRepositoryError::Internal(anyhow!(message))
}

fn invalid_data(error: impl std::error::Error + Send + Sync + 'static) -> MetadataRepositoryError {
    MetadataRepositoryError::Internal(anyhow!(error))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::{
        application::context::ExecutionContext,
        domain::actor::{
            ActorId, ActorKind, ActorRef, AuthenticationMode, DriveAccess, OrganizationId,
            RequestAuthContext,
        },
    };

    use super::public_organization_id;

    #[test]
    fn storage_tenant_projection_returns_the_drive_and_rejects_mismatches() -> anyhow::Result<()> {
        let owner = ActorRef::new(ActorKind::Carbon, ActorId::new("Ada1")?);
        let authorization = RequestAuthContext::for_drive(
            owner.clone(),
            owner,
            DriveAccess::Owner,
            AuthenticationMode::Bearer,
        )?;
        let execution = ExecutionContext::new(authorization, "storage-tenant-projection-test");
        assert_eq!(
            public_organization_id("Ada1", &execution)?,
            OrganizationId::new("Ada1")?
        );
        assert!(public_organization_id("ada1", &execution).is_err());
        assert!(public_organization_id(&format!("{}:Ada1", Uuid::now_v7()), &execution).is_err());
        Ok(())
    }
}
