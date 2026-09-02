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
use sqlx::{
    PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::{
    config::DatabaseSettings,
    domain::actor::{ActorKind, RequestAuthContext},
};

pub use content::PostgresContentRepository;
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
    actor_type: &'static str,
    actor_id: String,
    origin_app_id: Option<String>,
    request_id: String,
}

impl TenantContext {
    /// Creates database context from IAM-verified request facts.
    #[must_use]
    pub fn from_auth(auth: &RequestAuthContext, request_id: impl Into<String>) -> Self {
        let actor_type = match auth.actor().kind() {
            ActorKind::Carbon => "carbon",
            ActorKind::Silicon => "silicon",
        };
        Self {
            org_id: auth.organization_id().as_str().to_owned(),
            actor_type,
            actor_id: auth.actor().id().as_str().to_owned(),
            origin_app_id: auth
                .originating_application()
                .map(|application_id| application_id.as_str().to_owned()),
            request_id: request_id.into(),
        }
    }

    /// Creates database context for reading one member's own projection.
    ///
    /// An application request has an IAM-verified organization and actor but no
    /// request authority yet, so this constructor exists to read that member's
    /// projected role and tags under row-level security before authority is
    /// constructed. It grants nothing by itself.
    #[must_use]
    pub fn for_projection(
        org_id: String,
        actor: &crate::domain::actor::ActorRef,
        request_id: String,
    ) -> Self {
        let actor_type = match actor.kind() {
            ActorKind::Carbon => "carbon",
            ActorKind::Silicon => "silicon",
        };
        Self {
            org_id,
            actor_type,
            actor_id: actor.id().as_str().to_owned(),
            origin_app_id: None,
            request_id,
        }
    }

    /// Returns the authoritative organization identifier.
    #[must_use]
    pub fn org_id(&self) -> &str {
        &self.org_id
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
    install_transaction_context(
        &mut transaction,
        context.org_id(),
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
) -> Result<Transaction<'pool, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    install_transaction_context(
        &mut transaction,
        org_id,
        "service",
        "iam-webhook",
        None,
        request_id,
    )
    .await?;
    Ok(transaction)
}

async fn install_transaction_context(
    transaction: &mut Transaction<'_, Postgres>,
    org_id: &str,
    actor_type: &str,
    actor_id: &str,
    origin_app_id: Option<&str>,
    request_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('briefcase.org_id', $1, true), \
                set_config('briefcase.actor_type', $2, true), \
                set_config('briefcase.actor_id', $3, true), \
                set_config('briefcase.origin_app_id', $4, true), \
                set_config('briefcase.request_id', $5, true)",
    )
    .bind(org_id)
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
    use super::RoleCapabilities;

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
}
