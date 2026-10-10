//! Personal drives: their bookkeeping, root folder and apps folders.
//!
//! Every account owns one drive, the storage tenant its files live in. A
//! drive comes into being on the first request of its owner (or of the
//! custodian of the Silicon that owns it): the tenant row records the owner,
//! the owner and the caller become participants (every row names its actors
//! through a participant), and the root folder is created. Anyone else can
//! only reach a drive that already exists, and becomes a participant of it on
//! first use so what they do there can be recorded.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    application::service::MetadataRepositoryError,
    domain::actor::{ActorRef, RequestAuthContext},
};

use super::metadata::common::{Result, actor_kind, internal, map_sql};

const DRIVE_LOCK_NAMESPACE: i32 = 0x4252_4946;
/// The reserved name of the apps container at the top of every drive.
pub(super) const APPS_CONTAINER_NAME: &str = "apps";

#[derive(Debug, sqlx::FromRow)]
struct DriveState {
    /// The tenant row's recorded owner: `None` when no tenant row exists yet,
    /// an empty string for a pre-migration organization (no owner).
    recorded_owner: Option<String>,
    caller_participates: bool,
    owner_participates: bool,
    has_root: bool,
}

impl DriveState {
    const fn drive_exists(&self) -> bool {
        self.recorded_owner.is_some()
    }
}

/// Makes the request's drive usable for its caller, creating the drive on its
/// owner's (or custodian's) first request.
///
/// # Errors
///
/// Returns [`MetadataRepositoryError::NotFound`] when a caller without the
/// owner's rights names a drive that does not exist, or when the drive key
/// belongs to a pre-migration organization rather than an account.
pub(super) async fn ensure_participation(
    transaction: &mut Transaction<'_, Postgres>,
    authorization: &RequestAuthContext,
) -> Result<()> {
    let state = read_state(transaction, authorization).await?;
    if is_ready(&state, authorization)? {
        return Ok(());
    }
    lock_drive(transaction).await?;
    let state = read_state(transaction, authorization).await?;
    if !state.drive_exists() {
        if !authorization.has_owner_rights() {
            return Err(MetadataRepositoryError::NotFound);
        }
        sqlx::query(
            "INSERT INTO briefcase.organizations \
                 (org_id, iam_version, lifecycle_status, owner_account_uuid) \
             VALUES (briefcase.current_org_id(), 0, 'active', briefcase.current_org_id()) \
             ON CONFLICT (org_id) DO NOTHING",
        )
        .execute(&mut **transaction)
        .await
        .map_err(map_sql)?;
    } else if !records_its_owner(&state, authorization) {
        return Err(refuse_foreign_tenant());
    }
    let owner = authorization.drive_owner();
    if authorization.has_owner_rights() && !state.owner_participates {
        add_participant(transaction, owner, "owner").await?;
    }
    if !state.caller_participates {
        let role = if authorization.actor() == owner {
            "owner"
        } else {
            "member"
        };
        add_participant(transaction, authorization.actor(), role).await?;
    }
    if authorization.has_owner_rights() && !state.has_root {
        create_root(transaction, owner).await?;
    }
    Ok(())
}

/// Adds an account a share names as a participant of the request's drive, so
/// the grant can name it.
pub(super) async fn add_share_recipient(
    transaction: &mut Transaction<'_, Postgres>,
    recipient: &ActorRef,
) -> Result<()> {
    add_participant(transaction, recipient, "member").await
}

/// Returns the request drive's root folder.
pub(super) async fn root_id(transaction: &mut Transaction<'_, Postgres>) -> Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT entry_id FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() \
            AND system_kind = 'drive_root' AND deleted_at IS NULL",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)
}

/// Serializes structural changes of the request's drive.
pub(super) async fn lock_drive(transaction: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext(briefcase.current_org_id()), $1)")
        .bind(DRIVE_LOCK_NAMESPACE)
        .execute(&mut **transaction)
        .await
        .map_err(map_sql)?;
    Ok(())
}

/// Returns the app's folder `apps/{app}/` in the request's drive, creating
/// `apps/` and the folder on the app's first use. Both are system folders
/// owned by the drive's owner; the app's files go inside its folder.
pub(super) async fn ensure_application_folder(
    transaction: &mut Transaction<'_, Postgres>,
    authorization: &RequestAuthContext,
    application_id: &str,
) -> Result<Uuid> {
    if let Some(existing) = find_application_folder(transaction, application_id).await? {
        return Ok(existing);
    }
    if !authorization.has_owner_rights() {
        return Err(MetadataRepositoryError::NotFound);
    }
    lock_drive(transaction).await?;
    if let Some(existing) = find_application_folder(transaction, application_id).await? {
        return Ok(existing);
    }
    let root = root_id(transaction)
        .await?
        .ok_or_else(|| internal("drive has no root folder"))?;
    let owner = authorization.drive_owner();
    let container = match sqlx::query_scalar::<_, Uuid>(
        "SELECT entry_id FROM briefcase.entries \
          WHERE org_id = briefcase.current_org_id() AND system_kind = 'apps_root' \
            AND parent_id = $1 AND deleted_at IS NULL",
    )
    .bind(root)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)?
    {
        Some(container) => container,
        None => {
            insert_system_folder(
                transaction,
                root,
                APPS_CONTAINER_NAME,
                "apps_root",
                owner,
                None,
            )
            .await?
        }
    };
    insert_system_folder(
        transaction,
        container,
        application_id,
        "app_root",
        owner,
        Some(application_id),
    )
    .await
}

async fn find_application_folder(
    transaction: &mut Transaction<'_, Postgres>,
    application_id: &str,
) -> Result<Option<Uuid>> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT app.entry_id \
           FROM briefcase.entries AS root \
           JOIN briefcase.entries AS apps \
             ON apps.org_id = root.org_id AND apps.parent_id = root.entry_id \
           JOIN briefcase.entries AS app \
             ON app.org_id = apps.org_id AND app.parent_id = apps.entry_id \
          WHERE root.org_id = briefcase.current_org_id() \
            AND root.system_kind = 'drive_root' AND root.deleted_at IS NULL \
            AND apps.system_kind = 'apps_root' AND apps.name = $2 AND apps.deleted_at IS NULL \
            AND app.system_kind = 'app_root' AND app.name = $1 \
            AND app.origin_app_id = $1 AND app.deleted_at IS NULL",
    )
    .bind(application_id)
    .bind(APPS_CONTAINER_NAME)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql)
}

async fn insert_system_folder(
    transaction: &mut Transaction<'_, Postgres>,
    parent: Uuid,
    name: &str,
    system_kind: &str,
    owner: &ActorRef,
    application_id: Option<&str>,
) -> Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO briefcase.entries ( \
                org_id, entry_id, parent_id, entry_type, name, root_type, system_kind, \
                owner_type, owner_id, origin_app_id, created_by_type, created_by_id, \
                updated_by_type, updated_by_id \
         ) VALUES (briefcase.current_org_id(), $1, $2, 'folder', $3, 'private', $4, \
                   $5, $6, $7, $5, $6, $5, $6) \
         RETURNING entry_id",
    )
    .bind(Uuid::now_v7())
    .bind(parent)
    .bind(name)
    .bind(system_kind)
    .bind(actor_kind(owner.kind()))
    .bind(owner.id().as_str())
    .bind(application_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)
}

async fn read_state(
    transaction: &mut Transaction<'_, Postgres>,
    authorization: &RequestAuthContext,
) -> Result<DriveState> {
    let caller = authorization.actor();
    let owner = authorization.drive_owner();
    sqlx::query_as::<_, DriveState>(
        "SELECT (SELECT COALESCE(owner_account_uuid, '') FROM briefcase.organizations \
                  WHERE org_id = briefcase.current_org_id()) AS recorded_owner, \
                EXISTS (SELECT 1 FROM briefcase.organization_members \
                         WHERE org_id = briefcase.current_org_id() \
                           AND actor_type = $1 AND actor_id = $2) AS caller_participates, \
                EXISTS (SELECT 1 FROM briefcase.organization_members \
                         WHERE org_id = briefcase.current_org_id() \
                           AND actor_type = $3 AND actor_id = $4) AS owner_participates, \
                EXISTS (SELECT 1 FROM briefcase.entries \
                         WHERE org_id = briefcase.current_org_id() \
                           AND system_kind = 'drive_root' AND deleted_at IS NULL) AS has_root",
    )
    .bind(actor_kind(caller.kind()))
    .bind(caller.id().as_str())
    .bind(actor_kind(owner.kind()))
    .bind(owner.id().as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)
}

fn is_ready(state: &DriveState, authorization: &RequestAuthContext) -> Result<bool> {
    if !state.drive_exists() {
        return Ok(false);
    }
    if !records_its_owner(state, authorization) {
        return Err(refuse_foreign_tenant());
    }
    Ok(state.caller_participates
        && (!authorization.has_owner_rights() || (state.owner_participates && state.has_root)))
}

/// A drive's key is its owner's uuid. Account uuids are short base62
/// strings, so one can spell a pre-migration organization handle (`tos`):
/// such a tenant records no owner and is never entered as a drive.
fn records_its_owner(state: &DriveState, authorization: &RequestAuthContext) -> bool {
    state.recorded_owner.as_deref() == Some(authorization.organization_id().as_str())
}

fn refuse_foreign_tenant() -> MetadataRepositoryError {
    tracing::error!(
        event = "account_tenant_conflict",
        "a drive key names a tenant that is not that account's drive; refusing"
    );
    MetadataRepositoryError::NotFound
}

async fn add_participant(
    transaction: &mut Transaction<'_, Postgres>,
    participant: &ActorRef,
    role: &'static str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO briefcase.organization_members \
             (org_id, actor_type, actor_id, org_role, membership_status, iam_version) \
         VALUES (briefcase.current_org_id(), $1, $2, $3, 'active', 0) \
         ON CONFLICT (org_id, actor_type, actor_id) DO NOTHING",
    )
    .bind(actor_kind(participant.kind()))
    .bind(participant.id().as_str())
    .bind(role)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    Ok(())
}

async fn create_root(transaction: &mut Transaction<'_, Postgres>, owner: &ActorRef) -> Result<()> {
    sqlx::query(
        "INSERT INTO briefcase.entries ( \
                org_id, entry_id, parent_id, entry_type, name, root_type, system_kind, \
                owner_type, owner_id, created_by_type, created_by_id, updated_by_type, updated_by_id \
         ) VALUES (briefcase.current_org_id(), $1, NULL, 'folder', $2, 'private', 'drive_root', \
                   $3, $2, $3, $2, $3, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(owner.id().as_str())
    .bind(actor_kind(owner.kind()))
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    Ok(())
}

/// One entry another account shared with the caller, and the drive it is in.
#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
pub struct SharedEntry {
    /// The drive holding the entry (its owner's uuid).
    pub drive: String,
    /// The shared entry.
    pub entry_id: Uuid,
    /// When the newest live share on it was made.
    pub shared_at: time::OffsetDateTime,
}

impl super::PostgresRepository {
    /// Returns the drive holding an entry, active or in the bin, so a request
    /// for it can run inside that drive. Pre-migration organizations are never
    /// reported.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn entry_drive(
        &self,
        entry_id: Uuid,
    ) -> std::result::Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar::<_, Option<String>>("SELECT briefcase.entry_drive($1)")
            .bind(entry_id)
            .fetch_one(self.pool())
            .await
    }

    /// Lists entries other accounts shared with `principal`, newest first,
    /// one row per entry. Each must still be authorized inside its drive.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn shared_with(
        &self,
        principal: &ActorRef,
        before: Option<(time::OffsetDateTime, Uuid)>,
        limit: i32,
    ) -> std::result::Result<Vec<SharedEntry>, sqlx::Error> {
        sqlx::query_as::<_, SharedEntry>(
            "SELECT drive, entry_id, shared_at \
               FROM briefcase.shared_with_principal($1, $2, $3, $4, $5)",
        )
        .bind(actor_kind(principal.kind()))
        .bind(principal.id().as_str())
        .bind(before.map(|(at, _)| at))
        .bind(before.map(|(_, entry)| entry))
        .bind(limit)
        .fetch_all(self.pool())
        .await
    }

    /// Lists the other drives `principal` holds live shares in, most recently
    /// shared first.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn shared_drives(
        &self,
        principal: &ActorRef,
        limit: i32,
    ) -> std::result::Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>("SELECT drive FROM briefcase.principal_drives($1, $2, $3)")
            .bind(actor_kind(principal.kind()))
            .bind(principal.id().as_str())
            .bind(limit)
            .fetch_all(self.pool())
            .await
    }
}
