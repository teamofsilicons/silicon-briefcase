//! PostgreSQL connection management and tenant-scoped repository primitives.

mod content;
mod metadata;
mod models;
mod quota;
mod repository;
mod roots;
mod webhook;

use std::str::FromStr as _;

use anyhow::{Context as _, bail};
use secrecy::ExposeSecret as _;
use sha2::{Digest as _, Sha256};
use sqlx::{
    Connection as _, PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgConnection, PgPoolOptions},
};

use crate::{
    application::context::{ExecutionContext, TestingEnvironmentContext},
    config::DatabaseSettings,
    domain::actor::ActorKind,
};

pub use content::PostgresContentRepository;
pub(crate) use metadata::common::synchronize_iam_snapshot;
pub use models::{
    AccessRequestRow, AuditEventRow, EntryRow, EntryVersionRow, IdempotencyRecordRow,
    MultipartPartRow, MultipartUploadRow, OrganizationMemberRow, OrganizationRow,
    OrganizationStorageConfigRow, OrganizationTagRow, OutboxEventRow, PermissionGrantRow,
    SearchDocumentRow, WebhookReceiptRow,
};
pub use repository::{NewAuditEvent, NewOutboxEvent, PostgresRepository};

/// Request identity installed as transaction-local PostgreSQL settings.
///
/// Row-level security reads the organization setting. The remaining values are
/// available to repository code and database diagnostics without becoming
/// authorization inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantContext {
    org_id: String,
    testing_environment_id: Option<uuid::Uuid>,
    testing_environment_control_version: Option<i64>,
    actor_type: &'static str,
    actor_id: String,
    origin_app_id: Option<String>,
    request_id: String,
}

impl TenantContext {
    /// Creates database context from IAM-verified request facts.
    #[must_use]
    pub fn from_execution(execution: &ExecutionContext) -> Self {
        let auth = execution.authorization();
        let actor_type = match auth.actor().kind() {
            ActorKind::Carbon => "carbon",
            ActorKind::Silicon => "silicon",
        };
        let testing_environment = execution.testing_environment();
        Self {
            org_id: storage_org_id(auth.organization_id().as_str(), testing_environment),
            testing_environment_id: testing_environment.map(TestingEnvironmentContext::id),
            testing_environment_control_version: testing_environment
                .map(TestingEnvironmentContext::control_version),
            actor_type,
            actor_id: auth.actor().id().as_str().to_owned(),
            origin_app_id: auth
                .originating_application()
                .map(|application_id| application_id.as_str().to_owned()),
            request_id: execution.request_id().to_owned(),
        }
    }

    /// Creates a tenant context before an introspected principal UUID has been
    /// resolved to its public IAM actor identifier.
    #[must_use]
    pub fn for_token_projection(
        org_id: &str,
        actor_kind: ActorKind,
        principal_id: uuid::Uuid,
        request_id: String,
        testing_environment: Option<TestingEnvironmentContext>,
    ) -> Self {
        Self {
            org_id: storage_org_id(org_id, testing_environment),
            testing_environment_id: testing_environment.map(TestingEnvironmentContext::id),
            testing_environment_control_version: testing_environment
                .map(TestingEnvironmentContext::control_version),
            actor_type: match actor_kind {
                ActorKind::Carbon => "carbon",
                ActorKind::Silicon => "silicon",
            },
            actor_id: principal_id.to_string(),
            origin_app_id: None,
            request_id,
        }
    }

    /// Creates an internal context for a key-authorized sandbox lifecycle action.
    #[must_use]
    pub fn for_testing_environment_service(
        public_org_id: &str,
        testing_environment: TestingEnvironmentContext,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            org_id: storage_org_id(public_org_id, Some(testing_environment)),
            testing_environment_id: Some(testing_environment.id()),
            testing_environment_control_version: Some(testing_environment.control_version()),
            actor_type: "service",
            actor_id: "testing-environment".to_owned(),
            origin_app_id: None,
            request_id: request_id.into(),
        }
    }

    /// Creates an internal production control-plane context for one organization.
    #[must_use]
    pub fn for_control_service(public_org_id: &str, request_id: impl Into<String>) -> Self {
        Self {
            org_id: public_org_id.to_owned(),
            testing_environment_id: None,
            testing_environment_control_version: None,
            actor_type: "service",
            actor_id: "testing-environment".to_owned(),
            origin_app_id: None,
            request_id: request_id.into(),
        }
    }

    /// Returns the authoritative organization identifier.
    #[must_use]
    pub fn org_id(&self) -> &str {
        &self.org_id
    }

    /// Returns the selected test environment, if this is a sandbox request.
    #[must_use]
    pub const fn testing_environment_id(&self) -> Option<uuid::Uuid> {
        self.testing_environment_id
    }

    /// Returns the control-plane version authenticated with the environment.
    #[must_use]
    pub const fn testing_environment_control_version(&self) -> Option<i64> {
        self.testing_environment_control_version
    }

    /// Returns the represented actor type in its database encoding.
    #[must_use]
    pub const fn actor_type(&self) -> &'static str {
        self.actor_type
    }

    /// Returns the represented actor identifier.
    #[must_use]
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    /// Returns the IAM-verified originating application, if any.
    #[must_use]
    pub fn origin_app_id(&self) -> Option<&str> {
        self.origin_app_id.as_deref()
    }

    /// Returns the request correlation identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

fn storage_org_id(
    public_org_id: &str,
    testing_environment: Option<TestingEnvironmentContext>,
) -> String {
    testing_environment.map_or_else(
        || public_org_id.to_owned(),
        |environment| format!("{}:{public_org_id}", environment.id()),
    )
}

/// Creates and verifies a bounded PostgreSQL pool.
///
/// # Errors
///
/// Returns an error if the URL is malformed, the database is unavailable, or
/// required connection settings cannot be installed.
pub async fn connect(
    settings: &DatabaseSettings,
    application_name: &str,
) -> anyhow::Result<PgPool> {
    let options = PgConnectOptions::from_str(settings.url.expose_secret())?
        .application_name(application_name);
    let statement_timeout_ms = i64::try_from(settings.statement_timeout.as_millis())?;

    let pool = PgPoolOptions::new()
        .max_connections(settings.max_connections.get())
        .min_connections(settings.min_connections)
        .acquire_timeout(settings.acquire_timeout)
        .idle_timeout(Some(std::time::Duration::from_secs(300)))
        .max_lifetime(Some(std::time::Duration::from_mins(30)))
        .test_before_acquire(true)
        .after_connect(move |connection, _metadata| {
            Box::pin(async move {
                // PostgreSQL's default search path starts with a schema named
                // after the connecting role, and one runtime role is named
                // `briefcase` — the same name as the application schema. Every
                // statement Briefcase issues is schema-qualified, so the path
                // is pinned to `public` to keep unqualified names (notably the
                // migration bookkeeping table) resolving to one fixed schema
                // no matter which role connects.
                sqlx::query("SELECT set_config('search_path', 'public', false)")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SELECT set_config('timezone', 'UTC', false)")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SELECT set_config('statement_timeout', $1, false)")
                    .bind(statement_timeout_ms.to_string())
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;

    Ok(pool)
}

/// Executes all embedded forward migrations.
///
/// This function is intended for the dedicated migration process, not API or
/// worker startup.
///
/// # Errors
///
/// Returns an error if migration locking or any migration statement fails.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

/// Confirms that PostgreSQL can execute a trivial query.
pub async fn ready(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await
        .is_ok()
}

#[derive(Clone, Copy, Debug, sqlx::FromRow)]
struct RoleCapabilities {
    rolsuper: bool,
    rolbypassrls: bool,
}

impl RoleCapabilities {
    const fn bypasses_row_level_security(self) -> bool {
        self.rolsuper || self.rolbypassrls
    }
}

/// Verifies that an API connection cannot bypass tenant row-level security.
///
/// # Errors
///
/// Returns an error when the effective role cannot be inspected or has
/// superuser/`BYPASSRLS` authority.
pub async fn verify_tenant_isolated_role(pool: &PgPool) -> anyhow::Result<()> {
    let capabilities = role_capabilities(pool).await?;
    if capabilities.bypasses_row_level_security() {
        bail!("API database role must not be SUPERUSER or BYPASSRLS");
    }
    Ok(())
}

/// Verifies that a worker connection can intentionally scan all tenants.
///
/// # Errors
///
/// Returns an error when the effective role cannot be inspected or lacks both
/// superuser and `BYPASSRLS` authority.
pub async fn verify_cross_tenant_role(pool: &PgPool) -> anyhow::Result<()> {
    let capabilities = role_capabilities(pool).await?;
    if !capabilities.bypasses_row_level_security() {
        bail!("worker database role requires SUPERUSER or BYPASSRLS");
    }
    Ok(())
}

async fn role_capabilities(pool: &PgPool) -> anyhow::Result<RoleCapabilities> {
    sqlx::query_as::<_, RoleCapabilities>(
        "SELECT rolsuper, rolbypassrls \
           FROM pg_catalog.pg_roles \
          WHERE rolname = current_user",
    )
    .fetch_optional(pool)
    .await
    .context("database role inspection failed")?
    .ok_or_else(|| anyhow::anyhow!("database role could not be resolved"))
}

#[derive(Clone, Debug, Eq, PartialEq, sqlx::FromRow)]
struct DatabaseIdentity {
    system_identifier: String,
    database_oid: i64,
    database_name: String,
}

/// Confirms that production and testing pools resolve to different physical
/// PostgreSQL databases.
///
/// URI comparison is insufficient because aliases, alternate credentials, and
/// connection options can all address the same database. The cluster system
/// identifier plus database OID is stable across connections and roles, but
/// separately provisioned RDS clusters can inherit both from a common image.
/// Include the server-reported database name to distinguish those databases;
/// never rely on a caller's URI spelling, hostname or credentials.
///
/// # Errors
///
/// Returns an error when either identity cannot be read or both pools resolve
/// to the same database.
pub async fn verify_distinct_databases(
    production: &PgPool,
    testing: &PgPool,
) -> anyhow::Result<()> {
    let (production, testing) =
        tokio::try_join!(database_identity(production), database_identity(testing))?;
    ensure_distinct_database_identities(&production, &testing)
}

async fn database_identity(pool: &PgPool) -> anyhow::Result<DatabaseIdentity> {
    sqlx::query_as::<_, DatabaseIdentity>(
        "SELECT system_identifier, database_oid, \
                pg_catalog.current_database()::text AS database_name \
           FROM briefcase.database_identity()",
    )
    .fetch_one(pool)
    .await
    .context("database identity inspection failed")
}

fn ensure_distinct_database_identities(
    production: &DatabaseIdentity,
    testing: &DatabaseIdentity,
) -> anyhow::Result<()> {
    if production == testing {
        bail!("production and testing pools must resolve to different PostgreSQL databases");
    }
    Ok(())
}

/// Begins a transaction and installs request-local tenant identity for RLS.
///
/// Callers must perform all tenant table access through the returned
/// transaction. PostgreSQL automatically clears the settings on commit or
/// rollback, so pooled connections cannot retain authority from a prior
/// request.
///
/// # Errors
///
/// Returns an error when a connection cannot be acquired or transaction-local
/// settings cannot be installed.
pub async fn begin_tenant_transaction<'pool>(
    pool: &'pool PgPool,
    context: &TenantContext,
) -> Result<Transaction<'pool, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if let Some(environment_id) = context.testing_environment_id() {
        acquire_testing_environment_shared_transaction_lock(&mut transaction, environment_id)
            .await?;
    }
    install_transaction_context(
        &mut transaction,
        context.org_id(),
        context.testing_environment_id(),
        context.actor_type(),
        context.actor_id(),
        context.origin_app_id(),
        context.request_id(),
    )
    .await?;
    Ok(transaction)
}

pub(crate) async fn begin_projection_transaction<'pool>(
    pool: &'pool PgPool,
    org_id: &str,
    request_id: &str,
    testing_environment_id: Option<uuid::Uuid>,
) -> Result<Transaction<'pool, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if let Some(environment_id) = testing_environment_id {
        acquire_testing_environment_shared_transaction_lock(&mut transaction, environment_id)
            .await?;
    }
    install_transaction_context(
        &mut transaction,
        org_id,
        testing_environment_id,
        "service",
        "iam-webhook",
        None,
        request_id,
    )
    .await?;
    Ok(transaction)
}

/// Acquires the session-level exclusive side of an environment's lifecycle fence.
///
/// The caller must mark a pooled connection `close_on_drop` before acquiring
/// this lock. That makes cancellation fail safe: PostgreSQL closes the session
/// and releases the lock instead of returning a locked connection to the pool.
pub(crate) async fn acquire_testing_environment_exclusive_lock(
    connection: &mut PgConnection,
    environment_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(testing_environment_lock_key(environment_id))
        .execute(connection)
        .await?;
    Ok(())
}

/// Releases a session-level testing-environment lifecycle fence.
pub(crate) async fn release_testing_environment_exclusive_lock(
    connection: &mut PgConnection,
    environment_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(testing_environment_lock_key(environment_id))
        .fetch_one(connection)
        .await?;
    if !released {
        return Err(sqlx::Error::Protocol(
            "testing environment lifecycle fence was not held".to_owned(),
        ));
    }
    Ok(())
}

/// Acquires a session-level shared lifecycle fence for work that spans remote
/// IAM calls and therefore cannot live inside a database transaction.
pub(crate) async fn acquire_testing_environment_shared_session_lock(
    connection: &mut PgConnection,
    environment_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_lock_shared($1)")
        .bind(testing_environment_lock_key(environment_id))
        .execute(connection)
        .await?;
    Ok(())
}

/// Releases a session-level shared testing-environment lifecycle fence.
pub(crate) async fn release_testing_environment_shared_session_lock(
    connection: &mut PgConnection,
    environment_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock_shared($1)")
        .bind(testing_environment_lock_key(environment_id))
        .fetch_one(connection)
        .await?;
    if !released {
        return Err(sqlx::Error::Protocol(
            "testing environment shared lifecycle fence was not held".to_owned(),
        ));
    }
    Ok(())
}

/// Begins the destructive transaction beneath an already-held exclusive
/// testing-environment fence.
pub(crate) async fn begin_testing_environment_cleanup_transaction<'connection>(
    connection: &'connection mut PgConnection,
    context: &TenantContext,
) -> Result<Transaction<'connection, Postgres>, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    install_transaction_context(
        &mut transaction,
        context.org_id(),
        context.testing_environment_id(),
        context.actor_type(),
        context.actor_id(),
        context.origin_app_id(),
        context.request_id(),
    )
    .await?;
    Ok(transaction)
}

async fn acquire_testing_environment_shared_transaction_lock(
    transaction: &mut Transaction<'_, Postgres>,
    environment_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1)")
        .bind(testing_environment_lock_key(environment_id))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn testing_environment_lock_key(environment_id: uuid::Uuid) -> i64 {
    let mut digest = Sha256::new();
    digest.update(b"silicon-briefcase/testing-environment-clean-fence/v1");
    digest.update(environment_id.as_bytes());
    let digest = digest.finalize();
    let mut key = [0_u8; 8];
    key.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(key)
}

async fn install_transaction_context(
    transaction: &mut Transaction<'_, Postgres>,
    org_id: &str,
    testing_environment_id: Option<uuid::Uuid>,
    actor_type: &str,
    actor_id: &str,
    origin_app_id: Option<&str>,
    request_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('briefcase.org_id', $1, true), \
                set_config('briefcase.testing_environment_id', $2, true), \
                set_config('briefcase.actor_type', $3, true), \
                set_config('briefcase.actor_id', $4, true), \
                set_config('briefcase.origin_app_id', $5, true), \
                set_config('briefcase.request_id', $6, true)",
    )
    .bind(org_id)
    .bind(testing_environment_id.map_or_else(String::new, |id| id.to_string()))
    .bind(actor_type)
    .bind(actor_id)
    .bind(origin_app_id.unwrap_or_default())
    .bind(request_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DatabaseIdentity, RoleCapabilities, ensure_distinct_database_identities};

    #[test]
    fn role_capability_distinguishes_tenant_and_cross_tenant_principals() {
        assert!(
            RoleCapabilities {
                rolsuper: true,
                rolbypassrls: false,
            }
            .bypasses_row_level_security()
        );
        assert!(
            RoleCapabilities {
                rolsuper: false,
                rolbypassrls: true,
            }
            .bypasses_row_level_security()
        );
        assert!(
            !RoleCapabilities {
                rolsuper: false,
                rolbypassrls: false,
            }
            .bypasses_row_level_security()
        );
    }

    #[test]
    fn database_identity_rejects_aliases_for_the_same_database() {
        let production = DatabaseIdentity {
            system_identifier: "7483920011223344556".to_owned(),
            database_oid: 16_384,
            database_name: "briefcase".to_owned(),
        };
        let alias = production.clone();
        let testing = DatabaseIdentity {
            system_identifier: production.system_identifier.clone(),
            database_oid: production.database_oid + 1,
            database_name: "briefcase_test".to_owned(),
        };

        let rds_testing = DatabaseIdentity {
            database_name: "briefcase_test".to_owned(),
            ..production.clone()
        };

        assert!(ensure_distinct_database_identities(&production, &alias).is_err());
        assert!(ensure_distinct_database_identities(&production, &testing).is_ok());
        assert!(ensure_distinct_database_identities(&production, &rds_testing).is_ok());
    }
}
