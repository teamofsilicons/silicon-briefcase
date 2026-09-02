//! Idempotent materialization of the reserved organization filesystem roots.

use std::collections::BTreeSet;

use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::entry::MAX_ENTRY_NAME_BYTES;

const ROOT_RECONCILIATION_LOCK_NAMESPACE: i32 = 0x4252_4946;
const MAX_NAME_ATTEMPTS: u8 = 16;
const MALFORMED_SYSTEM_ROOT: &str = "malformed existing system root";

#[derive(Clone, Debug, sqlx::FromRow)]
struct ProjectedMember {
    actor_type: String,
    actor_id: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct ProjectedTag {
    tag_id: String,
    name: String,
    root_entry_id: Option<Uuid>,
    root_name: Option<String>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct RootSibling {
    entry_id: Uuid,
    name: String,
}

#[derive(Clone, Copy, Debug, sqlx::FromRow)]
struct RootPreflight {
    structurally_consistent: bool,
    tag_names_exact: bool,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct ExistingSystemRoot {
    entry_id: Uuid,
    parent_id: Option<Uuid>,
    entry_type: String,
    root_type: String,
    tag_id: Option<String>,
    system_kind: String,
    owner_type: String,
    owner_id: String,
    origin_app_id: Option<String>,
    content_type: Option<String>,
    size_bytes: Option<i64>,
    current_version_id: Option<Uuid>,
    deletion_batch_id: Option<Uuid>,
    deleted_at: Option<OffsetDateTime>,
    purge_after: Option<OffsetDateTime>,
    identity_exists: bool,
}

#[derive(Clone, Copy, Debug)]
struct SystemRootExpectation<'a> {
    system_kind: &'static str,
    root_type: &'static str,
    parent_id: Option<Uuid>,
    tag_id: Option<&'a str>,
    actor_owner: Option<(&'a str, &'a str)>,
}

#[derive(Clone, Debug)]
struct Custodian {
    actor_type: String,
    actor_id: String,
}

struct RootInsert<'a> {
    parent_id: Option<Uuid>,
    name: &'a str,
    root_type: &'static str,
    tag_id: Option<&'a str>,
    system_kind: &'static str,
    owner_type: &'a str,
    owner_id: &'a str,
}

/// Reconciles every system root derivable from the current IAM projection.
///
/// The caller may supply its freshly verified identity as the preferred
/// persistence custodian. System-container authorization never derives from
/// that custodial owner; actor roots remain owned by their represented member.
pub(super) async fn reconcile_system_roots(
    transaction: &mut Transaction<'_, Postgres>,
    preferred_custodian: Option<(&str, &str)>,
) -> Result<(), sqlx::Error> {
    lock_organization_reconciliation(transaction).await?;
    validate_existing_system_roots(transaction).await?;
    let Some(custodian) = resolve_custodian(transaction, preferred_custodian).await? else {
        // Organization-created webhooks can precede the first membership event.
        return Ok(());
    };

    ensure_singleton_root(
        transaction,
        "public_root",
        "public",
        "Public",
        "public",
        &custodian,
    )
    .await?;
    let private_root_id = ensure_singleton_root(
        transaction,
        "private_root",
        "private",
        "Private",
        "private",
        &custodian,
    )
    .await?;

    let active_tags = load_active_tags(transaction).await?;
    for tag in &active_tags {
        ensure_tag_root(transaction, tag, &custodian).await?;
    }

    let missing_members = sqlx::query_as::<_, ProjectedMember>(
        "SELECT member.actor_type, member.actor_id \
           FROM briefcase.organization_members AS member \
          WHERE member.org_id = briefcase.current_org_id() \
            AND member.membership_status = 'active' \
            AND NOT EXISTS ( \
                SELECT 1 FROM briefcase.entries AS entry \
                 WHERE entry.org_id = member.org_id \
                   AND entry.system_kind = 'actor_root' \
                   AND entry.entry_type = 'folder' \
                   AND entry.root_type = 'private' \
                   AND entry.parent_id = $1 \
                   AND entry.tag_id IS NULL \
                   AND entry.owner_type = member.actor_type \
                   AND entry.owner_id = member.actor_id \
                   AND entry.origin_app_id IS NULL \
                   AND entry.content_type IS NULL \
                   AND entry.size_bytes IS NULL \
                   AND entry.current_version_id IS NULL \
                   AND entry.deletion_batch_id IS NULL \
                   AND entry.deleted_at IS NULL \
                   AND entry.purge_after IS NULL \
            ) \
          ORDER BY member.actor_type, member.actor_id",
    )
    .bind(private_root_id)
    .fetch_all(&mut **transaction)
    .await?;
    for member in &missing_members {
        ensure_actor_root(transaction, private_root_id, member, &custodian).await?;
    }

    if let Some((actor_type, actor_id)) = preferred_custodian {
        // Online IAM is authoritative. This also repairs a caller root when a
        // delayed local membership projection has not yet returned to active.
        let caller = ProjectedMember {
            actor_type: actor_type.to_owned(),
            actor_id: actor_id.to_owned(),
        };
        ensure_actor_root(transaction, private_root_id, &caller, &custodian).await?;
    }

    Ok(())
}

/// Returns whether all roots needed by the current projection and caller exist.
///
/// This deliberately performs only reads so established organizations avoid
/// serializing every request on the reconciliation advisory lock.
#[allow(clippy::too_many_lines)]
pub(super) async fn system_roots_are_consistent(
    transaction: &mut Transaction<'_, Postgres>,
    caller: (&str, &str),
) -> Result<bool, sqlx::Error> {
    let preflight = sqlx::query_as::<_, RootPreflight>(
        "SELECT ( \
             EXISTS ( \
                 SELECT 1 FROM briefcase.organization_members AS caller \
                  WHERE caller.org_id = briefcase.current_org_id() \
                    AND caller.actor_type = $1 AND caller.actor_id = $2 \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM briefcase.entries AS reserved \
                  WHERE reserved.org_id = briefcase.current_org_id() \
                    AND reserved.system_kind IN ( \
                        'public_root', 'private_root', 'tag_root', 'actor_root' \
                    ) \
                    AND ( \
                        reserved.entry_type IS DISTINCT FROM 'folder' \
                        OR reserved.origin_app_id IS NOT NULL \
                        OR reserved.content_type IS NOT NULL \
                        OR reserved.size_bytes IS NOT NULL \
                        OR reserved.current_version_id IS NOT NULL \
                        OR reserved.deletion_batch_id IS NOT NULL \
                        OR reserved.deleted_at IS NOT NULL \
                        OR reserved.purge_after IS NOT NULL \
                        OR NOT EXISTS ( \
                            SELECT 1 FROM briefcase.organization_members AS owner \
                             WHERE owner.org_id = reserved.org_id \
                               AND owner.actor_type = reserved.owner_type \
                               AND owner.actor_id = reserved.owner_id \
                        ) \
                        OR EXISTS ( \
                            SELECT 1 FROM briefcase.entries AS duplicate \
                             WHERE duplicate.org_id = reserved.org_id \
                               AND duplicate.entry_id <> reserved.entry_id \
                               AND duplicate.system_kind = reserved.system_kind \
                               AND ( \
                                   reserved.system_kind IN ('public_root', 'private_root') \
                                   OR ( \
                                       reserved.system_kind = 'tag_root' \
                                       AND duplicate.tag_id = reserved.tag_id \
                                   ) \
                                   OR ( \
                                       reserved.system_kind = 'actor_root' \
                                       AND duplicate.owner_type = reserved.owner_type \
                                       AND duplicate.owner_id = reserved.owner_id \
                                   ) \
                               ) \
                        ) \
                        OR (CASE reserved.system_kind \
                            WHEN 'public_root' THEN \
                                reserved.root_type = 'public' \
                                AND reserved.parent_id IS NULL \
                                AND reserved.tag_id IS NULL \
                            WHEN 'private_root' THEN \
                                reserved.root_type = 'private' \
                                AND reserved.parent_id IS NULL \
                                AND reserved.tag_id IS NULL \
                            WHEN 'tag_root' THEN \
                                reserved.root_type = 'tag' \
                                AND reserved.parent_id IS NULL \
                                AND reserved.tag_id IS NOT NULL \
                                AND EXISTS ( \
                                    SELECT 1 FROM briefcase.organization_tags AS identity \
                                     WHERE identity.org_id = reserved.org_id \
                                       AND identity.tag_id = reserved.tag_id \
                                ) \
                            WHEN 'actor_root' THEN \
                                reserved.root_type = 'private' \
                                AND reserved.tag_id IS NULL \
                                AND EXISTS ( \
                                    SELECT 1 FROM briefcase.entries AS private \
                                     WHERE private.org_id = reserved.org_id \
                                       AND private.entry_id = reserved.parent_id \
                                       AND private.system_kind = 'private_root' \
                                       AND private.entry_type = 'folder' \
                                       AND private.root_type = 'private' \
                                       AND private.parent_id IS NULL \
                                       AND private.tag_id IS NULL \
                                       AND private.deleted_at IS NULL \
                                ) \
                            ELSE FALSE \
                        END) IS NOT TRUE \
                    ) \
             ) \
             AND EXISTS ( \
                 SELECT 1 FROM briefcase.entries AS public \
                  WHERE public.org_id = briefcase.current_org_id() \
                    AND public.system_kind = 'public_root' \
                    AND public.entry_type = 'folder' \
                    AND public.root_type = 'public' \
                    AND public.parent_id IS NULL AND public.tag_id IS NULL \
                    AND public.deleted_at IS NULL \
             ) \
             AND EXISTS ( \
                 SELECT 1 FROM briefcase.entries AS private \
                  WHERE private.org_id = briefcase.current_org_id() \
                    AND private.system_kind = 'private_root' \
                    AND private.entry_type = 'folder' \
                    AND private.root_type = 'private' \
                    AND private.parent_id IS NULL AND private.tag_id IS NULL \
                    AND private.deleted_at IS NULL \
             ) \
             AND EXISTS ( \
                 SELECT 1 FROM briefcase.entries AS actor_root \
                 JOIN briefcase.entries AS private \
                   ON private.org_id = actor_root.org_id \
                  AND private.entry_id = actor_root.parent_id \
                  AND private.system_kind = 'private_root' \
                  AND private.entry_type = 'folder' \
                  AND private.root_type = 'private' \
                  AND private.parent_id IS NULL AND private.tag_id IS NULL \
                  AND private.deleted_at IS NULL \
                  WHERE actor_root.org_id = briefcase.current_org_id() \
                    AND actor_root.system_kind = 'actor_root' \
                    AND actor_root.entry_type = 'folder' \
                    AND actor_root.root_type = 'private' \
                    AND actor_root.tag_id IS NULL \
                    AND actor_root.owner_type = $1 AND actor_root.owner_id = $2 \
                    AND actor_root.deleted_at IS NULL \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM briefcase.organization_members AS member \
                  WHERE member.org_id = briefcase.current_org_id() \
                    AND member.membership_status = 'active' \
                    AND NOT EXISTS ( \
                        SELECT 1 FROM briefcase.entries AS actor_root \
                        JOIN briefcase.entries AS private \
                          ON private.org_id = actor_root.org_id \
                         AND private.entry_id = actor_root.parent_id \
                         AND private.system_kind = 'private_root' \
                         AND private.entry_type = 'folder' \
                         AND private.root_type = 'private' \
                         AND private.parent_id IS NULL AND private.tag_id IS NULL \
                         AND private.deleted_at IS NULL \
                         WHERE actor_root.org_id = member.org_id \
                           AND actor_root.system_kind = 'actor_root' \
                           AND actor_root.entry_type = 'folder' \
                           AND actor_root.root_type = 'private' \
                           AND actor_root.tag_id IS NULL \
                           AND actor_root.owner_type = member.actor_type \
                           AND actor_root.owner_id = member.actor_id \
                           AND actor_root.deleted_at IS NULL \
                    ) \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM briefcase.organization_tags AS tag \
                  WHERE tag.org_id = briefcase.current_org_id() \
                    AND tag.lifecycle_status = 'active' \
                    AND NOT EXISTS ( \
                        SELECT 1 FROM briefcase.entries AS tag_root \
                         WHERE tag_root.org_id = tag.org_id \
                           AND tag_root.system_kind = 'tag_root' \
                           AND tag_root.entry_type = 'folder' \
                           AND tag_root.root_type = 'tag' \
                           AND tag_root.parent_id IS NULL \
                           AND tag_root.tag_id = tag.tag_id \
                           AND tag_root.origin_app_id IS NULL \
                           AND tag_root.content_type IS NULL \
                           AND tag_root.size_bytes IS NULL \
                           AND tag_root.current_version_id IS NULL \
                           AND tag_root.deletion_batch_id IS NULL \
                           AND tag_root.deleted_at IS NULL \
                           AND tag_root.purge_after IS NULL \
                    ) \
             ) \
         ) AS structurally_consistent, \
         NOT EXISTS ( \
             SELECT 1 FROM briefcase.organization_tags AS tag \
             JOIN briefcase.entries AS tag_root \
              ON tag_root.org_id = tag.org_id \
              AND tag_root.system_kind = 'tag_root' \
              AND tag_root.entry_type = 'folder' \
              AND tag_root.root_type = 'tag' \
              AND tag_root.parent_id IS NULL \
              AND tag_root.tag_id = tag.tag_id \
              AND tag_root.origin_app_id IS NULL \
              AND tag_root.content_type IS NULL \
              AND tag_root.size_bytes IS NULL \
              AND tag_root.current_version_id IS NULL \
              AND tag_root.deletion_batch_id IS NULL \
              AND tag_root.deleted_at IS NULL \
              AND tag_root.purge_after IS NULL \
              AND EXISTS ( \
                  SELECT 1 FROM briefcase.organization_members AS owner \
                   WHERE owner.org_id = tag_root.org_id \
                     AND owner.actor_type = tag_root.owner_type \
                     AND owner.actor_id = tag_root.owner_id \
              ) \
              WHERE tag.org_id = briefcase.current_org_id() \
                AND tag.lifecycle_status = 'active' \
                AND tag_root.name COLLATE \"C\" <> tag.name COLLATE \"C\" \
         ) AS tag_names_exact",
    )
    .bind(caller.0)
    .bind(caller.1)
    .fetch_one(&mut **transaction)
    .await?;
    if !preflight.structurally_consistent {
        return Ok(false);
    }
    if preflight.tag_names_exact {
        return Ok(true);
    }

    let active_tags = load_active_tags(transaction).await?;
    let root_siblings = sqlx::query_as::<_, RootSibling>(
        "SELECT entry_id, name FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() \
            AND parent_id IS NULL AND deleted_at IS NULL",
    )
    .fetch_all(&mut **transaction)
    .await?;

    Ok(active_tags
        .iter()
        .all(|tag| tag_root_name_is_current(tag, &root_siblings)))
}

async fn load_active_tags(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Vec<ProjectedTag>, sqlx::Error> {
    sqlx::query_as::<_, ProjectedTag>(
        "SELECT tag.tag_id, tag.name, \
                root.entry_id AS root_entry_id, root.name AS root_name \
           FROM briefcase.organization_tags AS tag \
           LEFT JOIN briefcase.entries AS root \
             ON root.org_id = tag.org_id \
            AND root.system_kind = 'tag_root' \
            AND root.entry_type = 'folder' \
            AND root.root_type = 'tag' \
            AND root.parent_id IS NULL \
            AND root.tag_id = tag.tag_id \
            AND root.origin_app_id IS NULL \
            AND root.content_type IS NULL \
            AND root.size_bytes IS NULL \
            AND root.current_version_id IS NULL \
            AND root.deletion_batch_id IS NULL \
            AND root.deleted_at IS NULL \
            AND root.purge_after IS NULL \
            AND EXISTS ( \
                SELECT 1 FROM briefcase.organization_members AS owner \
                 WHERE owner.org_id = root.org_id \
                   AND owner.actor_type = root.owner_type \
                   AND owner.actor_id = root.owner_id \
            ) \
          WHERE tag.org_id = briefcase.current_org_id() \
            AND tag.lifecycle_status = 'active' \
          ORDER BY tag.tag_id",
    )
    .fetch_all(&mut **transaction)
    .await
}

fn tag_root_name_is_current(tag: &ProjectedTag, root_siblings: &[RootSibling]) -> bool {
    let (Some(entry_id), Some(current_name)) = (tag.root_entry_id, tag.root_name.as_deref()) else {
        return false;
    };

    for attempt in 0..MAX_NAME_ATTEMPTS {
        let candidate = system_folder_name(&tag.name, "tag", entry_id, attempt);
        if candidate == current_name {
            return true;
        }
        if !root_siblings
            .iter()
            .any(|sibling| sibling.entry_id != entry_id && sibling.name == candidate)
        {
            // Reconciliation would choose this earlier available candidate.
            return false;
        }
    }

    false
}

pub(super) async fn lock_organization_reconciliation(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock( \
             hashtext(briefcase.current_org_id()), $1 \
         )",
    )
    .bind(ROOT_RECONCILIATION_LOCK_NAMESPACE)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn resolve_custodian(
    transaction: &mut Transaction<'_, Postgres>,
    preferred: Option<(&str, &str)>,
) -> Result<Option<Custodian>, sqlx::Error> {
    if let Some((actor_type, actor_id)) = preferred {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                 SELECT 1 FROM briefcase.organization_members \
                  WHERE org_id = briefcase.current_org_id() \
                    AND actor_type = $1 AND actor_id = $2 \
             )",
        )
        .bind(actor_type)
        .bind(actor_id)
        .fetch_one(&mut **transaction)
        .await?;
        if exists {
            return Ok(Some(Custodian {
                actor_type: actor_type.to_owned(),
                actor_id: actor_id.to_owned(),
            }));
        }
    }

    sqlx::query_as::<_, ProjectedMember>(
        "SELECT actor_type, actor_id \
           FROM briefcase.organization_members \
          WHERE org_id = briefcase.current_org_id() \
          ORDER BY CASE membership_status WHEN 'active' THEN 0 ELSE 1 END, \
                   CASE org_role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, \
                   created_at, actor_type, actor_id \
          LIMIT 1",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map(|member| {
        member.map(|member| Custodian {
            actor_type: member.actor_type,
            actor_id: member.actor_id,
        })
    })
}

async fn ensure_singleton_root(
    transaction: &mut Transaction<'_, Postgres>,
    system_kind: &'static str,
    root_type: &'static str,
    preferred_name: &str,
    name_namespace: &'static str,
    custodian: &Custodian,
) -> Result<Uuid, sqlx::Error> {
    if let Some(entry_id) = find_singleton_root(transaction, system_kind, root_type).await? {
        return Ok(entry_id);
    }

    for attempt in 0..MAX_NAME_ATTEMPTS {
        let entry_id = Uuid::now_v7();
        let name = system_folder_name(preferred_name, name_namespace, entry_id, attempt);
        let inserted = insert_root(
            transaction,
            entry_id,
            &RootInsert {
                parent_id: None,
                name: &name,
                root_type,
                tag_id: None,
                system_kind,
                owner_type: &custodian.actor_type,
                owner_id: &custodian.actor_id,
            },
            custodian,
        )
        .await?;
        if inserted {
            return Ok(entry_id);
        }
        if let Some(entry_id) = find_singleton_root(transaction, system_kind, root_type).await? {
            return Ok(entry_id);
        }
    }

    Err(reconciliation_error("system root name space exhausted"))
}

async fn ensure_tag_root(
    transaction: &mut Transaction<'_, Postgres>,
    tag: &ProjectedTag,
    custodian: &Custodian,
) -> Result<Uuid, sqlx::Error> {
    if let Some(entry_id) = tag.root_entry_id {
        let current_name = tag.root_name.as_deref().ok_or_else(|| {
            reconciliation_error("projected tag root is missing its display name")
        })?;
        reconcile_tag_root_name(transaction, tag, entry_id, current_name, custodian).await?;
        return Ok(entry_id);
    }

    for attempt in 0..MAX_NAME_ATTEMPTS {
        let entry_id = Uuid::now_v7();
        let name = system_folder_name(&tag.name, "tag", entry_id, attempt);
        let inserted = insert_root(
            transaction,
            entry_id,
            &RootInsert {
                parent_id: None,
                name: &name,
                root_type: "tag",
                tag_id: Some(&tag.tag_id),
                system_kind: "tag_root",
                owner_type: &custodian.actor_type,
                owner_id: &custodian.actor_id,
            },
            custodian,
        )
        .await?;
        if inserted {
            return Ok(entry_id);
        }
        if let Some(entry_id) = find_tag_root(transaction, &tag.tag_id).await? {
            return Ok(entry_id);
        }
    }

    Err(reconciliation_error("tag root name space exhausted"))
}

async fn reconcile_tag_root_name(
    transaction: &mut Transaction<'_, Postgres>,
    tag: &ProjectedTag,
    entry_id: Uuid,
    current_name: &str,
    custodian: &Custodian,
) -> Result<(), sqlx::Error> {
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let candidate = system_folder_name(&tag.name, "tag", entry_id, attempt);
        if candidate == current_name {
            return Ok(());
        }

        let renamed = sqlx::query_scalar::<_, Uuid>(
            "UPDATE briefcase.entries AS target \
                SET name = $2, updated_by_type = $3, updated_by_id = $4 \
              WHERE target.org_id = briefcase.current_org_id() \
                AND target.entry_id = $1 \
                AND target.system_kind = 'tag_root' \
                AND target.tag_id = $5 \
                AND target.deleted_at IS NULL \
                AND NOT EXISTS ( \
                    SELECT 1 FROM briefcase.entries AS sibling \
                     WHERE sibling.org_id = target.org_id \
                       AND sibling.parent_id IS NOT DISTINCT FROM target.parent_id \
                       AND sibling.entry_id <> target.entry_id \
                       AND sibling.deleted_at IS NULL \
                       AND sibling.name COLLATE \"C\" = $2 \
                ) \
            RETURNING target.entry_id",
        )
        .bind(entry_id)
        .bind(&candidate)
        .bind(&custodian.actor_type)
        .bind(&custodian.actor_id)
        .bind(&tag.tag_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if renamed.is_some() {
            return Ok(());
        }
    }

    Err(reconciliation_error("tag root rename name space exhausted"))
}

async fn ensure_actor_root(
    transaction: &mut Transaction<'_, Postgres>,
    private_root_id: Uuid,
    member: &ProjectedMember,
    custodian: &Custodian,
) -> Result<Uuid, sqlx::Error> {
    if let Some(entry_id) = find_actor_root(transaction, private_root_id, member).await? {
        return Ok(entry_id);
    }

    for attempt in 0..MAX_NAME_ATTEMPTS {
        let entry_id = Uuid::now_v7();
        let name = system_folder_name(&member.actor_id, &member.actor_type, entry_id, attempt);
        let inserted = insert_root(
            transaction,
            entry_id,
            &RootInsert {
                parent_id: Some(private_root_id),
                name: &name,
                root_type: "private",
                tag_id: None,
                system_kind: "actor_root",
                owner_type: &member.actor_type,
                owner_id: &member.actor_id,
            },
            custodian,
        )
        .await?;
        if inserted {
            return Ok(entry_id);
        }
        if let Some(entry_id) = find_actor_root(transaction, private_root_id, member).await? {
            return Ok(entry_id);
        }
    }

    Err(reconciliation_error("actor root name space exhausted"))
}

async fn insert_root(
    transaction: &mut Transaction<'_, Postgres>,
    entry_id: Uuid,
    root: &RootInsert<'_>,
    custodian: &Custodian,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO briefcase.entries ( \
                org_id, entry_id, parent_id, entry_type, name, root_type, tag_id, system_kind, \
                owner_type, owner_id, created_by_type, created_by_id, updated_by_type, updated_by_id \
         ) VALUES (briefcase.current_org_id(), $1, $2, 'folder', $3, $4, $5, $6, \
                   $7, $8, $9, $10, $9, $10) \
         ON CONFLICT DO NOTHING \
         RETURNING entry_id",
    )
    .bind(entry_id)
    .bind(root.parent_id)
    .bind(root.name)
    .bind(root.root_type)
    .bind(root.tag_id)
    .bind(root.system_kind)
    .bind(root.owner_type)
    .bind(root.owner_id)
    .bind(&custodian.actor_type)
    .bind(&custodian.actor_id)
    .fetch_optional(&mut **transaction)
    .await
    .map(|inserted| inserted.is_some())
}

async fn find_singleton_root(
    transaction: &mut Transaction<'_, Postgres>,
    system_kind: &'static str,
    root_type: &'static str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if !matches!(
        (system_kind, root_type),
        ("public_root", "public") | ("private_root", "private")
    ) {
        return Err(malformed_system_root_error());
    }
    find_system_root(
        transaction,
        SystemRootExpectation {
            system_kind,
            root_type,
            parent_id: None,
            tag_id: None,
            actor_owner: None,
        },
    )
    .await
}

async fn find_tag_root(
    transaction: &mut Transaction<'_, Postgres>,
    tag_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    find_system_root(
        transaction,
        SystemRootExpectation {
            system_kind: "tag_root",
            root_type: "tag",
            parent_id: None,
            tag_id: Some(tag_id),
            actor_owner: None,
        },
    )
    .await
}

/// Name of the reserved folder that holds one member's application data.
const APPLICATION_CONTAINER_NAME: &str = "apps";

/// Materializes `private/{actor}/apps/{app_id}` for one member and application.
///
/// The product contract gives every application its own folder inside the
/// represented member's private folder, and that is where app-created files
/// land by default. Both levels are reserved system folders, so nobody can
/// rename, move, delete, or share them out from under the application.
pub(super) async fn ensure_application_container(
    transaction: &mut Transaction<'_, Postgres>,
    actor: (&str, &str),
    application_id: &str,
) -> Result<Uuid, sqlx::Error> {
    lock_organization_reconciliation(transaction).await?;
    let member = ProjectedMember {
        actor_type: actor.0.to_owned(),
        actor_id: actor.1.to_owned(),
    };
    let custodian = Custodian {
        actor_type: member.actor_type.clone(),
        actor_id: member.actor_id.clone(),
    };
    let private_root_id = find_system_root(
        transaction,
        SystemRootExpectation {
            system_kind: "private_root",
            root_type: "private",
            parent_id: None,
            tag_id: None,
            actor_owner: None,
        },
    )
    .await?
    .ok_or_else(|| reconciliation_error("private root is missing"))?;
    let actor_root_id =
        ensure_actor_root(transaction, private_root_id, &member, &custodian).await?;

    let apps_id = ensure_container(
        transaction,
        actor_root_id,
        APPLICATION_CONTAINER_NAME,
        &member,
        &custodian,
    )
    .await?;
    ensure_container(transaction, apps_id, application_id, &member, &custodian).await
}

/// Finds or creates one reserved application folder below a parent.
async fn ensure_container(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
    name: &str,
    member: &ProjectedMember,
    custodian: &Custodian,
) -> Result<Uuid, sqlx::Error> {
    if let Some(entry_id) = find_container(transaction, parent_id, name, member).await? {
        return Ok(entry_id);
    }
    let entry_id = Uuid::now_v7();
    let inserted = insert_root(
        transaction,
        entry_id,
        &RootInsert {
            parent_id: Some(parent_id),
            name,
            root_type: "private",
            tag_id: None,
            system_kind: "app_container",
            owner_type: &member.actor_type,
            owner_id: &member.actor_id,
        },
        custodian,
    )
    .await?;
    if inserted {
        return Ok(entry_id);
    }
    // A concurrent request created it, or a user folder already owns the name.
    find_container(transaction, parent_id, name, member)
        .await?
        .ok_or_else(|| reconciliation_error("application container name is taken"))
}

async fn find_container(
    transaction: &mut Transaction<'_, Postgres>,
    parent_id: Uuid,
    name: &str,
    member: &ProjectedMember,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT entry_id FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() \
            AND parent_id = $1 AND name = $2 \
            AND entry_type = 'folder' AND root_type = 'private' \
            AND system_kind = 'app_container' \
            AND owner_type = $3 AND owner_id = $4 \
            AND deleted_at IS NULL",
    )
    .bind(parent_id)
    .bind(name)
    .bind(&member.actor_type)
    .bind(&member.actor_id)
    .fetch_optional(&mut **transaction)
    .await
}

async fn find_actor_root(
    transaction: &mut Transaction<'_, Postgres>,
    private_root_id: Uuid,
    member: &ProjectedMember,
) -> Result<Option<Uuid>, sqlx::Error> {
    find_system_root(
        transaction,
        SystemRootExpectation {
            system_kind: "actor_root",
            root_type: "private",
            parent_id: Some(private_root_id),
            tag_id: None,
            actor_owner: Some((&member.actor_type, &member.actor_id)),
        },
    )
    .await
}

async fn find_system_root(
    transaction: &mut Transaction<'_, Postgres>,
    expected: SystemRootExpectation<'_>,
) -> Result<Option<Uuid>, sqlx::Error> {
    let root = sqlx::query_as::<_, ExistingSystemRoot>(
        "SELECT root.entry_id, root.parent_id, root.entry_type, root.root_type, \
                root.tag_id, root.system_kind, root.owner_type, root.owner_id, \
                root.origin_app_id, root.content_type, root.size_bytes, \
                root.current_version_id, root.deletion_batch_id, root.deleted_at, \
                root.purge_after, ( \
                    EXISTS ( \
                        SELECT 1 FROM briefcase.organization_members AS owner \
                         WHERE owner.org_id = root.org_id \
                           AND owner.actor_type = root.owner_type \
                           AND owner.actor_id = root.owner_id \
                    ) \
                    AND (root.system_kind <> 'tag_root' OR EXISTS ( \
                        SELECT 1 FROM briefcase.organization_tags AS tag \
                         WHERE tag.org_id = root.org_id AND tag.tag_id = root.tag_id \
                    )) \
                ) AS identity_exists \
           FROM briefcase.entries AS root \
          WHERE root.org_id = briefcase.current_org_id() \
            AND root.system_kind = $1 \
            AND ($2::text IS NULL OR root.tag_id = $2) \
            AND ($3::text IS NULL OR (root.owner_type = $3 AND root.owner_id = $4))",
    )
    .bind(expected.system_kind)
    .bind(expected.tag_id)
    .bind(expected.actor_owner.map(|owner| owner.0))
    .bind(expected.actor_owner.map(|owner| owner.1))
    .fetch_optional(&mut **transaction)
    .await?;
    match root {
        Some(root) if system_root_shape_matches(&root, expected) => Ok(Some(root.entry_id)),
        Some(_) => Err(malformed_system_root_error()),
        None => Ok(None),
    }
}

async fn validate_existing_system_roots(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    let roots = sqlx::query_as::<_, ExistingSystemRoot>(
        "SELECT root.entry_id, root.parent_id, root.entry_type, root.root_type, \
                root.tag_id, root.system_kind, root.owner_type, root.owner_id, \
                root.origin_app_id, root.content_type, root.size_bytes, \
                root.current_version_id, root.deletion_batch_id, root.deleted_at, \
                root.purge_after, ( \
                    EXISTS ( \
                        SELECT 1 FROM briefcase.organization_members AS owner \
                         WHERE owner.org_id = root.org_id \
                           AND owner.actor_type = root.owner_type \
                           AND owner.actor_id = root.owner_id \
                    ) \
                    AND (root.system_kind <> 'tag_root' OR EXISTS ( \
                        SELECT 1 FROM briefcase.organization_tags AS tag \
                         WHERE tag.org_id = root.org_id AND tag.tag_id = root.tag_id \
                    )) \
                ) AS identity_exists \
           FROM briefcase.entries AS root \
          WHERE root.org_id = briefcase.current_org_id() \
            AND root.system_kind IN ( \
                'public_root', 'private_root', 'tag_root', 'actor_root' \
            ) \
          ORDER BY root.system_kind, root.entry_id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    if system_root_set_is_valid(&roots) {
        Ok(())
    } else {
        Err(malformed_system_root_error())
    }
}

fn system_root_set_is_valid(roots: &[ExistingSystemRoot]) -> bool {
    let mut private_root_id = None;
    for root in roots
        .iter()
        .filter(|root| root.system_kind == "private_root")
    {
        if private_root_id.replace(root.entry_id).is_some()
            || !system_root_shape_matches(
                root,
                SystemRootExpectation {
                    system_kind: "private_root",
                    root_type: "private",
                    parent_id: None,
                    tag_id: None,
                    actor_owner: None,
                },
            )
        {
            return false;
        }
    }

    let mut public_root_seen = false;
    let mut tag_identities = BTreeSet::new();
    let mut actor_identities = BTreeSet::new();
    for root in roots {
        let expectation = match root.system_kind.as_str() {
            "public_root" => {
                if public_root_seen {
                    return false;
                }
                public_root_seen = true;
                SystemRootExpectation {
                    system_kind: "public_root",
                    root_type: "public",
                    parent_id: None,
                    tag_id: None,
                    actor_owner: None,
                }
            }
            "private_root" => continue,
            "tag_root" => {
                let Some(tag_id) = root.tag_id.as_deref() else {
                    return false;
                };
                if !tag_identities.insert(tag_id) {
                    return false;
                }
                SystemRootExpectation {
                    system_kind: "tag_root",
                    root_type: "tag",
                    parent_id: None,
                    tag_id: Some(tag_id),
                    actor_owner: None,
                }
            }
            "actor_root" => {
                let Some(private_root_id) = private_root_id else {
                    return false;
                };
                let identity = (root.owner_type.as_str(), root.owner_id.as_str());
                if !actor_identities.insert(identity) {
                    return false;
                }
                SystemRootExpectation {
                    system_kind: "actor_root",
                    root_type: "private",
                    parent_id: Some(private_root_id),
                    tag_id: None,
                    actor_owner: Some(identity),
                }
            }
            _ => return false,
        };
        if !system_root_shape_matches(root, expectation) {
            return false;
        }
    }

    true
}

fn system_root_shape_matches(
    root: &ExistingSystemRoot,
    expected: SystemRootExpectation<'_>,
) -> bool {
    root.identity_exists
        && root.deleted_at.is_none()
        && root.purge_after.is_none()
        && root.deletion_batch_id.is_none()
        && root.entry_type == "folder"
        && root.origin_app_id.is_none()
        && root.content_type.is_none()
        && root.size_bytes.is_none()
        && root.current_version_id.is_none()
        && root.system_kind == expected.system_kind
        && root.root_type == expected.root_type
        && root.parent_id == expected.parent_id
        && root.tag_id.as_deref() == expected.tag_id
        && expected.actor_owner.is_none_or(|(owner_type, owner_id)| {
            root.owner_type == owner_type && root.owner_id == owner_id
        })
}

fn system_folder_name(preferred: &str, namespace: &str, entry_id: Uuid, attempt: u8) -> String {
    let mut base = preferred
        .trim()
        .chars()
        .map(|character| {
            if character == '/' || character == '\0' || character.is_control() {
                '-'
            } else {
                character
            }
        })
        .collect::<String>();
    if base.is_empty() || matches!(base.as_str(), "." | "..") {
        base.clear();
        base.push_str("Folder");
    }

    let suffix = collision_suffix(namespace, entry_id, attempt);
    let maximum_base_bytes = MAX_ENTRY_NAME_BYTES.saturating_sub(suffix.len());
    truncate_utf8(&mut base, maximum_base_bytes);
    while base.chars().next_back().is_some_and(char::is_whitespace) {
        base.pop();
    }
    if base.is_empty() {
        base.push_str("Folder");
    }
    base.push_str(&suffix);
    base
}

fn collision_suffix(namespace: &str, entry_id: Uuid, attempt: u8) -> String {
    if attempt == 0 {
        return String::new();
    }

    let identifier = entry_id.simple().to_string();
    let short_identifier = &identifier[identifier.len() - 12..];
    if attempt == 1 {
        format!(" [{namespace}-{short_identifier}]")
    } else {
        format!(" [{namespace}-{short_identifier}-{attempt}]")
    }
}

fn truncate_utf8(value: &mut String, maximum_bytes: usize) {
    if value.len() <= maximum_bytes {
        return;
    }
    let mut boundary = maximum_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn reconciliation_error(message: &'static str) -> sqlx::Error {
    sqlx::Error::Protocol(message.to_owned())
}

fn malformed_system_root_error() -> sqlx::Error {
    reconciliation_error(MALFORMED_SYSTEM_ROOT)
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::domain::entry::{EntryName, MAX_ENTRY_NAME_BYTES};

    use super::{
        ExistingSystemRoot, ProjectedTag, RootSibling, system_folder_name,
        system_root_set_is_valid, tag_root_name_is_current,
    };

    fn system_root(
        entry_id: u128,
        system_kind: &str,
        root_type: &str,
        parent_id: Option<u128>,
        tag_id: Option<&str>,
        owner_id: &str,
    ) -> ExistingSystemRoot {
        ExistingSystemRoot {
            entry_id: Uuid::from_u128(entry_id),
            parent_id: parent_id.map(Uuid::from_u128),
            entry_type: "folder".to_owned(),
            root_type: root_type.to_owned(),
            tag_id: tag_id.map(str::to_owned),
            system_kind: system_kind.to_owned(),
            owner_type: "carbon".to_owned(),
            owner_id: owner_id.to_owned(),
            origin_app_id: None,
            content_type: None,
            size_bytes: None,
            current_version_id: None,
            deletion_batch_id: None,
            deleted_at: None,
            purge_after: None,
            identity_exists: true,
        }
    }

    fn valid_system_roots() -> Vec<ExistingSystemRoot> {
        vec![
            system_root(1, "public_root", "public", None, None, "custodian"),
            system_root(2, "private_root", "private", None, None, "custodian"),
            system_root(3, "tag_root", "tag", None, Some("tag-finance"), "custodian"),
            system_root(4, "actor_root", "private", Some(2), None, "carbon-a"),
        ]
    }

    #[test]
    fn complete_reserved_root_shapes_are_accepted() {
        assert!(system_root_set_is_valid(&valid_system_roots()));
    }

    #[test]
    fn malformed_reserved_root_shapes_fail_closed() {
        let valid = valid_system_roots();

        let mut deleted = valid.clone();
        deleted[0].deleted_at = Some(OffsetDateTime::UNIX_EPOCH);
        assert!(!system_root_set_is_valid(&deleted));

        let mut file = valid.clone();
        file[1].entry_type = "file".to_owned();
        assert!(!system_root_set_is_valid(&file));

        let mut file_metadata = valid.clone();
        file_metadata[1].content_type = Some("application/octet-stream".to_owned());
        assert!(!system_root_set_is_valid(&file_metadata));

        let mut wrong_tag_boundary = valid.clone();
        wrong_tag_boundary[2].root_type = "private".to_owned();
        assert!(!system_root_set_is_valid(&wrong_tag_boundary));

        let mut misparented_actor = valid.clone();
        misparented_actor[3].parent_id = Some(Uuid::from_u128(99));
        assert!(!system_root_set_is_valid(&misparented_actor));

        let mut missing_identity = valid;
        missing_identity[2].identity_exists = false;
        assert!(!system_root_set_is_valid(&missing_identity));
    }

    #[test]
    fn duplicate_reserved_identities_fail_closed() {
        let mut roots = valid_system_roots();
        roots.push(system_root(
            5,
            "tag_root",
            "tag",
            None,
            Some("tag-finance"),
            "custodian",
        ));

        assert!(!system_root_set_is_valid(&roots));
    }

    #[test]
    fn canonical_root_names_are_preserved_when_available() {
        let identifier = Uuid::from_u128(1);

        assert_eq!(
            system_folder_name("Public", "public", identifier, 0),
            "Public"
        );
        assert_eq!(
            system_folder_name("Private", "private", identifier, 0),
            "Private"
        );
        assert_eq!(
            system_folder_name("finance", "tag", identifier, 0),
            "finance"
        );
    }

    #[test]
    fn collision_names_are_valid_bounded_and_unique() {
        let first = system_folder_name(
            &"é".repeat(MAX_ENTRY_NAME_BYTES),
            "carbon",
            Uuid::from_u128(1),
            1,
        );
        let second = system_folder_name(
            &"é".repeat(MAX_ENTRY_NAME_BYTES),
            "carbon",
            Uuid::from_u128(2),
            1,
        );

        assert_ne!(first, second);
        assert!(first.len() <= MAX_ENTRY_NAME_BYTES);
        assert!(second.len() <= MAX_ENTRY_NAME_BYTES);
        assert!(EntryName::new(&first).is_ok());
        assert!(EntryName::new(&second).is_ok());
    }

    #[test]
    fn repeated_collisions_have_stable_distinct_candidates() {
        let identifier = Uuid::from_u128(42);
        let first = system_folder_name("finance", "tag", identifier, 1);
        let second = system_folder_name("finance", "tag", identifier, 2);

        assert_eq!(first, system_folder_name("finance", "tag", identifier, 1));
        assert_ne!(first, second);
        assert!(EntryName::new(first).is_ok());
        assert!(EntryName::new(second).is_ok());
    }

    #[test]
    fn tag_name_preflight_accepts_only_the_first_available_candidate() {
        let root_id = Uuid::from_u128(42);
        let fallback = system_folder_name("finance", "tag", root_id, 1);
        let tag = ProjectedTag {
            tag_id: "tag-finance".to_owned(),
            name: "finance".to_owned(),
            root_entry_id: Some(root_id),
            root_name: Some(fallback.clone()),
        };
        let root = RootSibling {
            entry_id: root_id,
            name: fallback,
        };

        assert!(!tag_root_name_is_current(&tag, std::slice::from_ref(&root)));

        let occupied_canonical = RootSibling {
            entry_id: Uuid::from_u128(7),
            name: "finance".to_owned(),
        };
        assert!(tag_root_name_is_current(&tag, &[root, occupied_canonical]));
    }

    #[test]
    fn tag_name_preflight_detects_renames_and_accepts_sanitized_names() {
        let root_id = Uuid::from_u128(42);
        let renamed = ProjectedTag {
            tag_id: "tag-finance".to_owned(),
            name: "accounting".to_owned(),
            root_entry_id: Some(root_id),
            root_name: Some("finance".to_owned()),
        };
        assert!(!tag_root_name_is_current(
            &renamed,
            &[RootSibling {
                entry_id: root_id,
                name: "finance".to_owned(),
            }]
        ));

        let sanitized = ProjectedTag {
            tag_id: "tag-research".to_owned(),
            name: "team/research".to_owned(),
            root_entry_id: Some(root_id),
            root_name: Some("team-research".to_owned()),
        };
        assert!(tag_root_name_is_current(
            &sanitized,
            &[RootSibling {
                entry_id: root_id,
                name: "team-research".to_owned(),
            }]
        ));
    }

    #[test]
    fn external_identifier_path_characters_are_never_folder_paths() {
        let name = system_folder_name("team/research", "tag", Uuid::from_u128(3), 0);

        assert_eq!(name, "team-research");
        assert!(EntryName::new(name).is_ok());
    }
}
