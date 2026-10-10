//! The cross-drive functions under production's database roles.
//!
//! Production applies migrations as a role that owns the schema but is
//! neither a superuser nor exempt from row security (`briefcase_migrator`),
//! and serves requests as `briefcase_api` (NOBYPASSRLS). Every request runs
//! inside one drive; the few questions that span drives are answered by
//! SECURITY DEFINER functions that run as the schema's owner, so they work
//! only while the migration's definer policies let that owner read across
//! drives — something a superuser test database never shows. This test
//! migrates a fresh database as such an owner and calls every function as the
//! API role, then fires the drive-root trigger as the worker role.
//!
//! Skipped unless `BRIEFCASE_TEST_DATABASE_URL` names a superuser connection
//! (it creates a role and a throwaway database, and drops the database).
//!
//! ```bash
//! BRIEFCASE_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5460/briefcase_mig \
//!   cargo test --test postgres_drive_roles
//! ```

use std::str::FromStr as _;

use silicon_briefcase::infrastructure::postgres;
use sqlx::{
    Connection as _, Executor as _, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use uuid::Uuid;

const MIGRATOR: &str = "briefcase_test_migrator";
const MIGRATOR_PASSWORD: &str = "briefcase-test-migrator-local-only";

/// A throwaway database owned by a non-superuser, non-BYPASSRLS migrator.
struct RoleWorld {
    admin: PgPool,
    database: String,
    owner: PgConnectOptions,
}

impl RoleWorld {
    async fn create(url: &str) -> anyhow::Result<Option<Self>> {
        let options = PgConnectOptions::from_str(url)?;
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await?;
        let superuser: bool = sqlx::query_scalar(
            "SELECT rolsuper FROM pg_catalog.pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&admin)
        .await?;
        let runtime_roles: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_catalog.pg_roles \
              WHERE rolname IN ('briefcase_api', 'briefcase_worker')",
        )
        .fetch_one(&admin)
        .await?;
        if !superuser || runtime_roles != 2 {
            eprintln!(
                "skipping: needs a superuser connection and the runtime roles from \
                 deploy/postgres/001_runtime_roles.sql"
            );
            return Ok(None);
        }
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                    "DO $$ BEGIN \
                       IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = '{MIGRATOR}') THEN \
                         CREATE ROLE {MIGRATOR} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; \
                       END IF; \
                     END $$"
                ))
            )
            .await?;
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                "ALTER ROLE {MIGRATOR} PASSWORD '{MIGRATOR_PASSWORD}'"
            )))
            .await?;
        let database = format!(
            "briefcase_roles_{}",
            &Uuid::now_v7().simple().to_string()[16..]
        );
        admin
            .execute(sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE {database} OWNER {MIGRATOR}"
            )))
            .await?;
        let owner = options
            .clone()
            .database(&database)
            .username(MIGRATOR)
            .password(MIGRATOR_PASSWORD);
        Ok(Some(Self {
            admin,
            database,
            owner,
        }))
    }

    /// A superuser session in the throwaway database that has switched to a
    /// runtime role, exactly as that role would see the schema.
    async fn session_as(&self, role: &str) -> anyhow::Result<PgConnection> {
        let options = (*self.admin.connect_options())
            .clone()
            .database(&self.database);
        let mut connection = PgConnection::connect_with(&options).await?;
        connection
            .execute(sqlx::AssertSqlSafe(format!("SET ROLE {role}")))
            .await?;
        Ok(connection)
    }

    async fn drop(self) -> anyhow::Result<()> {
        self.admin
            .execute(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.database
            )))
            .await?;
        self.admin.close().await;
        Ok(())
    }
}

async fn in_drive(connection: &mut PgConnection, drive: &str) -> anyhow::Result<()> {
    sqlx::query("SELECT set_config('briefcase.org_id', $1, false)")
        .bind(drive)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn count(connection: &mut PgConnection, sql: &'static str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(&mut *connection)
        .await?)
}

/// Ada's drive with its root, a folder shared with Bob, and the participants
/// the rows name.
async fn seed_drive(
    api: &mut PgConnection,
    ada: &str,
    bob: &str,
    root: Uuid,
    folder: Uuid,
) -> anyhow::Result<()> {
    in_drive(api, ada).await?;
    sqlx::query(
        "INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status, owner_account_uuid) \
         VALUES ($1, 0, 'active', $1)",
    )
    .bind(ada)
    .execute(&mut *api)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.organization_members \
             (org_id, actor_type, actor_id, org_role, membership_status, iam_version) \
         VALUES ($1, 'carbon', $1, 'owner', 'active', 0), ($1, 'carbon', $2, 'member', 'active', 0)",
    )
    .bind(ada)
    .bind(bob)
    .execute(&mut *api)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.entries (org_id, entry_id, parent_id, entry_type, name, root_type, \
                system_kind, owner_type, owner_id, created_by_type, created_by_id, updated_by_type, \
                updated_by_id) \
         VALUES ($1, $2, NULL, 'folder', $1, 'private', 'drive_root', 'carbon', $1, 'carbon', $1, \
                 'carbon', $1), \
                ($1, $3, $2, 'folder', 'plans', 'private', NULL, 'carbon', $1, 'carbon', $1, \
                 'carbon', $1)",
    )
    .bind(ada)
    .bind(root)
    .bind(folder)
    .execute(&mut *api)
    .await?;
    sqlx::query(
        "INSERT INTO briefcase.permission_grants (org_id, entry_id, grant_id, principal_type, \
                principal_id, access_mask, inherits_to_descendants, granted_by_type, granted_by_id) \
         VALUES ($1, $2, $3, 'carbon', $4, 1, true, 'carbon', $1)",
    )
    .bind(ada)
    .bind(folder)
    .bind(Uuid::now_v7())
    .bind(bob)
    .execute(&mut *api)
    .await?;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn cross_drive_functions_work_for_the_api_under_a_migrator_without_bypass()
-> anyhow::Result<()> {
    let Ok(url) = std::env::var("BRIEFCASE_TEST_DATABASE_URL") else {
        eprintln!("skipping: BRIEFCASE_TEST_DATABASE_URL is not set");
        return Ok(());
    };
    let Some(world) = RoleWorld::create(&url).await? else {
        return Ok(());
    };
    let outcome = exercise(&world).await;
    world.drop().await?;
    outcome
}

#[allow(clippy::too_many_lines)]
async fn exercise(world: &RoleWorld) -> anyhow::Result<()> {
    // Migrate exactly as production does: as the schema's owner, without
    // superuser and without BYPASSRLS.
    let owner = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(world.owner.clone())
        .await?;
    let bypasses: bool = sqlx::query_scalar(
        "SELECT rolsuper OR rolbypassrls FROM pg_catalog.pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&owner)
    .await?;
    anyhow::ensure!(!bypasses, "the migrator must not bypass row security");
    postgres::migrate(&owner).await?;
    owner.close().await;

    let (ada, bob) = ("AdaRole1", "BobRole1");
    let (root, folder) = (Uuid::now_v7(), Uuid::now_v7());
    let mut api = world.session_as("briefcase_api").await?;
    seed_drive(&mut api, ada, bob, root, folder).await?;

    // A notification lands in the recipient's own inbox, creating that
    // drive's bookkeeping, and the caller's drive setting is restored.
    let delivered: bool = sqlx::query_scalar(
        "SELECT briefcase.deliver_notification('carbon', $1, 'access_granted', 'carbon', $2, $3, \
                jsonb_build_object('name', 'plans', 'path', 'plans', 'entry_type', 'folder', \
                                   'drive', $2::text))",
    )
    .bind(bob)
    .bind(ada)
    .bind(folder)
    .fetch_one(&mut api)
    .await?;
    assert!(delivered);
    let setting: String = sqlx::query_scalar("SELECT current_setting('briefcase.org_id')")
        .fetch_one(&mut api)
        .await?;
    assert_eq!(setting, ada, "the caller's drive is restored");
    assert_eq!(
        count(&mut api, "SELECT count(*) FROM briefcase.notifications").await?,
        0,
        "Ada's drive holds none of Bob's inbox"
    );

    // Inside Bob's drive: his inbox, nothing of Ada's rows, and the
    // cross-drive answers about what Ada shared with him.
    in_drive(&mut api, bob).await?;
    assert_eq!(
        count(&mut api, "SELECT count(*) FROM briefcase.notifications").await?,
        1
    );
    assert_eq!(
        count(&mut api, "SELECT count(*) FROM briefcase.outbox_events").await?,
        1,
        "the invitation email is queued in the recipient's drive"
    );
    assert_eq!(
        count(&mut api, "SELECT count(*) FROM briefcase.entries").await?,
        0,
        "row security still confines Bob to his own drive"
    );
    let drive: Option<String> = sqlx::query_scalar("SELECT briefcase.entry_drive($1)")
        .bind(folder)
        .fetch_one(&mut api)
        .await?;
    assert_eq!(drive.as_deref(), Some(ada));
    let unknown: Option<String> = sqlx::query_scalar("SELECT briefcase.entry_drive($1)")
        .bind(Uuid::now_v7())
        .fetch_one(&mut api)
        .await?;
    assert_eq!(unknown, None);
    let shared: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT drive, entry_id FROM briefcase.shared_with_principal('carbon', $1, NULL, NULL, 10)",
    )
    .bind(bob)
    .fetch_all(&mut api)
    .await?;
    assert_eq!(shared, vec![(ada.to_owned(), folder)]);
    let drives: Vec<String> =
        sqlx::query_scalar("SELECT drive FROM briefcase.principal_drives('carbon', $1, 10)")
            .bind(bob)
            .fetch_all(&mut api)
            .await?;
    assert_eq!(drives, vec![ada.to_owned()]);

    // A pre-migration organization (no recorded owner) is never a drive:
    // nothing is delivered into it and none of its entries is reported.
    in_drive(&mut api, "tos").await?;
    sqlx::query(
        "INSERT INTO briefcase.organizations (org_id, iam_version, lifecycle_status) \
         VALUES ('tos', 0, 'active')",
    )
    .execute(&mut api)
    .await?;
    let legacy: bool = sqlx::query_scalar(
        "SELECT briefcase.deliver_notification('carbon', 'tos', 'access_granted', NULL, NULL, NULL, '{}')",
    )
    .fetch_one(&mut api)
    .await?;
    assert!(!legacy, "a legacy organization is not an inbox");

    // account.deleted: shares to the account end in every drive, and the
    // account's own drive is erased. Both refuse an account that is not
    // recorded as deleted.
    let refused = sqlx::query("SELECT briefcase.erase_deleted_account_drive($1, 'req-active')")
        .bind(ada)
        .execute(&mut api)
        .await;
    assert!(
        refused.is_err_and(|error| error.to_string().contains("only for a deleted account")),
        "an active account's drive is never erased"
    );
    sqlx::query(
        "INSERT INTO briefcase.accounts (account_uuid, kind, status, deleted_at) \
         VALUES ($1, 'carbon', 'deleted', clock_timestamp()), \
                ($2, 'carbon', 'deleted', clock_timestamp())",
    )
    .bind(ada)
    .bind(bob)
    .execute(&mut api)
    .await?;
    in_drive(&mut api, bob).await?;
    let revoked: i64 =
        sqlx::query_scalar("SELECT briefcase.revoke_deleted_account_shares('carbon', $1, 'req-1')")
            .bind(bob)
            .fetch_one(&mut api)
            .await?;
    assert_eq!(revoked, 1);
    let erased: i64 =
        sqlx::query_scalar("SELECT briefcase.erase_deleted_account_drive($1, 'req-2')")
            .bind(ada)
            .fetch_one(&mut api)
            .await?;
    assert_eq!(erased, 2, "Ada's root and folder are erased");
    in_drive(&mut api, ada).await?;
    assert_eq!(
        count(
            &mut api,
            "SELECT count(*) FROM briefcase.entries \
              WHERE deleted_at IS NULL OR purge_after > clock_timestamp()"
        )
        .await?,
        0,
        "everything in the deleted account's drive is due for purge"
    );
    assert_eq!(
        count(
            &mut api,
            "SELECT count(*) FROM briefcase.permission_grants WHERE revoked_at IS NULL"
        )
        .await?,
        0
    );
    assert_eq!(
        count(
            &mut api,
            "SELECT count(*) FROM briefcase.audit_events \
              WHERE action IN ('permission.revoked_account_deleted.v1', \
                               'drive.erased_account_deleted.v1')"
        )
        .await?,
        2,
        "both erasures are audited in Ada's drive"
    );
    drop(api);

    // The worker (BYPASSRLS) moving an expired self-destructing file to the
    // bin fires the drive-root name trigger, which it must be allowed to run.
    let mut worker = world.session_as("briefcase_worker").await?;
    let touched = sqlx::query(
        "UPDATE briefcase.entries \
            SET deletion_batch_id = briefcase.new_uuid_v7(), deleted_at = clock_timestamp(), \
                purge_after = clock_timestamp() \
          WHERE org_id = $1 AND entry_id = $2",
    )
    .bind(ada)
    .bind(folder)
    .execute(&mut worker)
    .await?;
    assert_eq!(touched.rows_affected(), 1);
    Ok(())
}
