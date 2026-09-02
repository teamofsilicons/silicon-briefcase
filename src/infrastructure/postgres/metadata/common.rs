use std::collections::BTreeSet;

use anyhow::anyhow;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
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
        actor::{
            ActorId, ActorKind, ActorRef, ApplicationId, OrganizationId, OrganizationRole, TagName,
        },
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

#[derive(Clone, Debug, sqlx::FromRow)]
struct ProjectedCaller {
    org_role: String,
    membership_status: String,
    tag_names: Vec<String>,
}

pub(in crate::infrastructure::postgres) async fn begin<'pool>(
    repository: &'pool PostgresRepository,
    execution: &ExecutionContext,
) -> Result<RequestTransaction<'pool>> {
    let context =
        TenantContext::from_auth(execution.authorization(), execution.request_id().to_owned());
    let mut transaction = repository.begin(&context).await.map_err(map_sql)?;
    let authorization = execution.authorization();
    let actor = authorization.actor();
    let caller = (actor_kind(actor.kind()), actor.id().as_str());
    let caller_is_current = caller_projection_is_current(&mut transaction, authorization).await?;
    let roots_are_consistent = caller_is_current
        && super::super::roots::system_roots_are_consistent(&mut transaction, caller)
            .await
            .map_err(map_sql)?;
    if !caller_is_current || !roots_are_consistent {
        super::super::roots::lock_organization_reconciliation(&mut transaction)
            .await
            .map_err(map_sql)?;
        let caller_is_current =
            caller_projection_is_current(&mut transaction, authorization).await?;
        let roots_are_consistent = caller_is_current
            && super::super::roots::system_roots_are_consistent(&mut transaction, caller)
                .await
                .map_err(map_sql)?;
        if !caller_is_current {
            refresh_caller_projection(&mut transaction, execution).await?;
        }
        if !caller_is_current || !roots_are_consistent {
            super::super::roots::reconcile_system_roots(&mut transaction, Some(caller))
                .await
                .map_err(map_sql)?;
        }
    }
    Ok(RequestTransaction {
        context,
        transaction,
    })
}

async fn caller_projection_is_current(
    transaction: &mut Transaction<'_, Postgres>,
    authorization: &crate::domain::actor::RequestAuthContext,
) -> Result<bool> {
    let actor = authorization.actor();
    let projected = sqlx::query_as::<_, ProjectedCaller>(
        "SELECT member.org_role, member.membership_status, \
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
          GROUP BY member.org_role, member.membership_status",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?;

    Ok(projected.as_ref().is_some_and(|projected| {
        caller_projection_matches(projected, authorization.role(), authorization.tags())
    }))
}

fn caller_projection_matches(
    projected: &ProjectedCaller,
    current_role: OrganizationRole,
    current_tags: &BTreeSet<TagName>,
) -> bool {
    if projected.membership_status != "active"
        || projected.org_role != organization_role(current_role)
    {
        return false;
    }

    let projected_tags = projected
        .tag_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let current_tags = current_tags
        .iter()
        .map(TagName::as_str)
        .collect::<BTreeSet<_>>();
    projected_tags == current_tags
}

async fn refresh_caller_projection(
    transaction: &mut Transaction<'_, Postgres>,
    execution: &ExecutionContext,
) -> Result<()> {
    let authorization = execution.authorization();
    let actor = authorization.actor();
    sqlx::query(
        "INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status) \
         VALUES (briefcase.current_org_id(), 0, 'active') \
         ON CONFLICT (org_id) DO NOTHING",
    )
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    // Online IAM supplies current authorization facts but no aggregate version.
    // Refresh those facts without lowering the webhook high-water mark.
    let member_version = sqlx::query_scalar::<_, i64>(
        "INSERT INTO briefcase.organization_members ( \
                org_id, actor_type, actor_id, org_role, membership_status, iam_version \
         ) VALUES (briefcase.current_org_id(), $1, $2, $3, 'active', 0) \
         ON CONFLICT (org_id, actor_type, actor_id) DO UPDATE \
             SET org_role = EXCLUDED.org_role, membership_status = 'active' \
         RETURNING iam_version",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(organization_role(authorization.role()))
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;

    let mut tag_ids = BTreeSet::new();
    for tag in authorization.tags() {
        tag_ids.insert(ensure_online_tag(transaction, tag.as_str()).await?);
    }
    sqlx::query(
        "DELETE FROM briefcase.organization_member_tags \
          WHERE org_id = briefcase.current_org_id() \
            AND actor_type = $1 AND actor_id = $2",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    for tag_id in tag_ids {
        sqlx::query(
            "INSERT INTO briefcase.organization_member_tags ( \
                    org_id, actor_type, actor_id, tag_id, iam_version \
             ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4)",
        )
        .bind(actor_kind(actor.kind()))
        .bind(actor.id().as_str())
        .bind(tag_id)
        .bind(member_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql)?;
    }
    Ok(())
}

async fn ensure_online_tag(
    transaction: &mut Transaction<'_, Postgres>,
    tag_name: &str,
) -> Result<String> {
    // Name is the only tag identity carried by online IAM. Prefer a projected
    // canonical row and preserve its webhook version and stable tag ID.
    if let Some(tag_id) = sqlx::query_scalar::<_, String>(
        "UPDATE briefcase.organization_tags \
            SET lifecycle_status = 'active' \
          WHERE org_id = briefcase.current_org_id() AND name = $1 \
         RETURNING tag_id",
    )
    .bind(tag_name)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?
    {
        return Ok(tag_id);
    }

    // A missing online name is projected at version zero. A same-ID row already
    // owned by a webhook version cannot be safely reidentified and fails closed.
    sqlx::query_scalar::<_, String>(
        "INSERT INTO briefcase.organization_tags ( \
                org_id, tag_id, name, lifecycle_status, iam_version \
         ) VALUES (briefcase.current_org_id(), $1, $1, 'active', 0) \
         ON CONFLICT (org_id, tag_id) DO UPDATE \
             SET name = EXCLUDED.name, lifecycle_status = 'active' \
           WHERE organization_tags.iam_version = 0 \
         RETURNING tag_id",
    )
    .bind(tag_name)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?
    .ok_or(MetadataRepositoryError::Conflict)
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
    let required_for_traversal = if row.deleted_at.is_some() {
        false
    } else {
        has_visible_descendant(transaction, execution, row.entry_id).await?
    };
    Ok(AuthorizableEntry {
        entry: entry_view(&row, tag_name.as_deref())?,
        system_kind: system_kind(row.system_kind.as_deref())?,
        grants,
        required_for_traversal,
    })
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
        depth: i32,
    }

    let actor = execution.authorization().actor();
    let rows = sqlx::query_as::<_, ResolvedGrantRow>(
        "SELECT grant.org_id, grant.entry_id, grant.grant_id, grant.principal_type, \
                grant.principal_id, grant.access_mask, grant.inherits_to_descendants, \
                grant.granted_by_type, grant.granted_by_id, grant.revoked_at, \
                grant.revoked_by_type, grant.revoked_by_id, grant.created_at, path.depth \
           FROM briefcase.entry_closure AS path \
           JOIN briefcase.permission_grants AS grant \
             ON grant.org_id = path.org_id AND grant.entry_id = path.ancestor_id \
          WHERE path.org_id = briefcase.current_org_id() \
            AND path.descendant_id = $1 \
            AND grant.principal_type = $2 AND grant.principal_id = $3 \
            AND grant.revoked_at IS NULL \
            AND (path.depth = 0 OR grant.inherits_to_descendants) \
          ORDER BY path.depth, grant.grant_id",
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
            };
            Ok(ResolvedPermissionGrant {
                grant: permission_grant(grant_row)?,
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
    if execution.authorization().role().has_administrative_access() {
        return Ok(false);
    }
    let actor = execution.authorization().actor();
    let tags: Vec<&str> = execution
        .authorization()
        .tags()
        .iter()
        .map(TagName::as_str)
        .collect();
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
             SELECT 1 \
               FROM briefcase.entry_closure AS subtree \
               JOIN briefcase.entries AS child \
                 ON child.org_id = subtree.org_id AND child.entry_id = subtree.descendant_id \
          LEFT JOIN briefcase.organization_tags AS tag \
                 ON tag.org_id = child.org_id AND tag.tag_id = child.tag_id \
              WHERE subtree.org_id = briefcase.current_org_id() \
                AND subtree.ancestor_id = $1 AND subtree.depth > 0 \
                AND child.deleted_at IS NULL \
                AND ( \
                    (child.owner_type = $2 AND child.owner_id = $3) \
                    OR child.root_type = 'public' \
                    OR (child.root_type = 'tag' AND tag.name = ANY($4)) \
                    OR EXISTS ( \
                        SELECT 1 \
                          FROM briefcase.entry_closure AS grant_path \
                          JOIN briefcase.permission_grants AS grant \
                            ON grant.org_id = grant_path.org_id \
                           AND grant.entry_id = grant_path.ancestor_id \
                         WHERE grant_path.org_id = child.org_id \
                           AND grant_path.descendant_id = child.entry_id \
                           AND grant.principal_type = $2 AND grant.principal_id = $3 \
                           AND grant.revoked_at IS NULL \
                           AND (grant_path.depth = 0 OR grant.inherits_to_descendants) \
                    ) \
                ) \
         )",
    )
    .bind(entry_id)
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(tags)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)
}

pub(in crate::infrastructure::postgres) fn entry_view(
    row: &EntryRow,
    tag_name: Option<&str>,
) -> Result<EntryView> {
    let organization_id = OrganizationId::new(row.org_id.clone()).map_err(invalid_data)?;
    let boundary = match row.root_type.as_str() {
        "public" => EntryBoundary::Public,
        "private" => EntryBoundary::Private,
        "tag" => EntryBoundary::Tag {
            tag: TagName::new(tag_name.ok_or_else(|| internal("missing entry tag projection"))?)
                .map_err(invalid_data)?,
        },
        _ => return Err(internal("invalid entry root type")),
    };
    Ok(EntryView {
        id: EntryId::from_uuid(row.entry_id).map_err(invalid_data)?,
        organization_id,
        kind: entry_kind(&row.entry_type)?,
        name: EntryName::new(&row.name).map_err(invalid_data)?,
        path: EntryPath::new(&row.path).map_err(invalid_data)?,
        parent_id: row
            .parent_id
            .map(EntryId::from_uuid)
            .transpose()
            .map_err(invalid_data)?,
        boundary,
        owner: actor_ref(&row.owner_type, &row.owner_id)?,
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
    })
}

pub(in crate::infrastructure::postgres) fn permission_grant(
    row: PermissionGrantRow,
) -> Result<PermissionGrant> {
    Ok(PermissionGrant::from_parts(PermissionGrantParts {
        id: GrantId::from_uuid(row.grant_id).map_err(invalid_data)?,
        organization_id: OrganizationId::new(row.org_id).map_err(invalid_data)?,
        entry_id: EntryId::from_uuid(row.entry_id).map_err(invalid_data)?,
        principal: actor_ref(&row.principal_type, &row.principal_id)?,
        access: decode_access(row.access_mask)?,
        inheritance: PermissionInheritance::from_inherit_flag(row.inherits_to_descendants),
        granted_by: actor_ref(&row.granted_by_type, &row.granted_by_id)?,
        created_at: row.created_at,
    }))
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

pub(in crate::infrastructure::postgres) const fn organization_role(
    role: OrganizationRole,
) -> &'static str {
    match role {
        OrganizationRole::Member => "member",
        OrganizationRole::Admin => "admin",
        OrganizationRole::Owner => "owner",
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
        Some("public_root") => Ok(Some(SystemEntryKind::PublicContainer)),
        Some("private_root") => Ok(Some(SystemEntryKind::PrivateContainer)),
        Some("tag_root") => Ok(Some(SystemEntryKind::TagRoot)),
        Some("actor_root" | "app_container") => Ok(Some(SystemEntryKind::PrivateActorFolder)),
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
    use std::collections::BTreeSet;

    use crate::domain::actor::{OrganizationRole, TagName};

    use super::{ProjectedCaller, caller_projection_matches};

    fn tag(value: &str) -> TagName {
        match TagName::new(value) {
            Ok(tag) => tag,
            Err(error) => panic!("test tag must be valid: {error}"),
        }
    }

    #[test]
    fn caller_projection_tag_comparison_is_order_independent() {
        let projected = ProjectedCaller {
            org_role: "admin".to_owned(),
            membership_status: "active".to_owned(),
            tag_names: vec!["research".to_owned(), "finance".to_owned()],
        };
        let current = [tag("finance"), tag("research")]
            .into_iter()
            .collect::<BTreeSet<_>>();

        assert!(caller_projection_matches(
            &projected,
            OrganizationRole::Admin,
            &current
        ));
    }

    #[test]
    fn caller_projection_requires_active_role_and_exact_tag_set() {
        let projected = ProjectedCaller {
            org_role: "member".to_owned(),
            membership_status: "removed".to_owned(),
            tag_names: vec!["finance".to_owned()],
        };
        let duplicate_input = [tag("finance"), tag("finance")]
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(duplicate_input.len(), 1);
        assert!(!caller_projection_matches(
            &projected,
            OrganizationRole::Member,
            &duplicate_input
        ));

        let active = ProjectedCaller {
            membership_status: "active".to_owned(),
            ..projected
        };
        assert!(!caller_projection_matches(
            &active,
            OrganizationRole::Owner,
            &duplicate_input
        ));
        assert!(!caller_projection_matches(
            &active,
            OrganizationRole::Member,
            &[tag("finance"), tag("research")].into_iter().collect()
        ));
    }
}
