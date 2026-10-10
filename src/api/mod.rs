//! HTTP transport and process lifecycle.

use std::{future::IntoFuture as _, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, header},
    middleware as axum_middleware,
    routing::{delete, get, post, put},
};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    catch_panic::CatchPanicLayer,
    sensitive_headers::{SetSensitiveRequestHeadersLayer, SetSensitiveResponseHeadersLayer},
};

use crate::{
    application::{
        content::{ContentRepository, ContentService},
        delegated_upload::{DelegatedUploadRepository, DelegatedUploadService},
        ports::ObjectStore,
        service::{MetadataRepository, MetadataService},
    },
    config::{ServerSettings, Settings, WebhookSettings},
    infrastructure::{
        accounts::{AccountDirectory, AccountsGateway},
        postgres::{self, PostgresContentRepository, PostgresRepository, accounts::AccountStore},
        s3::S3ObjectStore,
    },
};

mod auth;
pub mod cursor;
mod delivery;
mod drives;
pub mod dto;
mod extract;
mod handlers;
pub(crate) mod mapping;
mod middleware;
mod state;
pub mod upload;
pub mod validation;
pub mod versioning;

use handlers::{
    accounts, content, delegated, delegated_upload, drives as drive_handlers, entries, invitations,
    notifications, obo, permissions, reports, sharing, system, telemetry, usage,
};
use state::{AppState, ContentUseCases, DelegatedUploadUseCases};

/// Builds the dependency graph, binds the configured listener, and serves the
/// complete HTTP contract until graceful shutdown.
///
/// # Errors
///
/// Returns an error when a required adapter cannot initialize, the listener
/// cannot bind, or the server exits unexpectedly.
pub async fn serve(settings: Settings) -> anyhow::Result<()> {
    let database = postgres::connect(&settings.database, "briefcase-api").await?;
    postgres::verify_tenant_isolated_role(&database).await?;
    let object_store: Arc<dyn ObjectStore> =
        Arc::new(S3ObjectStore::from_settings(&settings.s3).await);
    let repository = PostgresRepository::new(database.clone());
    let metadata_repository: Arc<dyn MetadataRepository> = Arc::new(repository.clone());
    let concrete_content = Arc::new(PostgresContentRepository::new(
        repository,
        settings.s3.clone(),
    ));
    let content_repository: Arc<dyn ContentRepository> = concrete_content.clone();
    let delegated_repository: Arc<dyn DelegatedUploadRepository> = concrete_content.clone();
    let lease_seconds = settings
        .server
        .upload_timeout
        .checked_add(settings.s3.operation_timeout)
        .and_then(|duration| duration.checked_add(Duration::from_secs(120)))
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .ok_or_else(|| anyhow::anyhow!("delegated upload lease exceeds supported duration"))?;
    let delegated_uploads: Arc<DelegatedUploadUseCases> = Arc::new(DelegatedUploadService::new(
        delegated_repository,
        Arc::clone(&object_store),
        lease_seconds,
        settings.s3.operation_timeout,
    ));
    let content: Arc<ContentUseCases> = Arc::new(ContentService::new(
        content_repository,
        Arc::clone(&object_store),
        settings.s3.temporary_directory.clone(),
    ));
    let gateway = AccountsGateway::new(&settings.accounts)
        .map_err(|error| anyhow::anyhow!("Silicon Accounts client: {error}"))?;
    let accounts = AccountDirectory::new(
        Arc::new(gateway),
        AccountStore::new(database.clone()),
        &settings.accounts,
    );
    if settings.accounts.proof_issuers.is_empty() {
        tracing::warn!(
            event = "proof_issuers_unset",
            "BRIEFCASE_PROOF_ISSUERS is empty: no app may act for an account through /api/v1/obo/*"
        );
    }
    let state = AppState {
        accounts: Arc::new(accounts),
        metadata: MetadataService::new(metadata_repository),
        content,
        content_adapter: concrete_content,
        objects: object_store,
        delegated_uploads,
        database: database.clone(),
        contract_database: Some(database.clone()),
        mapper: mapping::ResponseMapper::new(
            &settings.server.public_base_url,
            settings.server.public_site_base_url.clone(),
        ),
        temporary_directory: settings.s3.temporary_directory.clone(),
        webhook_settings: settings.webhook.clone(),
    };
    let application = router(state, &settings.server, &settings.webhook);
    let listener = tokio::net::TcpListener::bind(settings.server.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Briefcase API listening");

    serve_until_shutdown(
        listener,
        application,
        database,
        settings.server.shutdown_timeout,
    )
    .await
}

async fn serve_until_shutdown(
    listener: tokio::net::TcpListener,
    application: Router,
    database: sqlx::PgPool,
    shutdown_timeout: Duration,
) -> anyhow::Result<()> {
    let (shutdown_sender, mut shutdown_receiver) = tokio::sync::watch::channel(false);
    let graceful_shutdown = async move {
        while !*shutdown_receiver.borrow_and_update() {
            if shutdown_receiver.changed().await.is_err() {
                return;
            }
        }
    };
    let server = axum::serve(listener, application)
        .with_graceful_shutdown(graceful_shutdown)
        .into_future();
    tokio::pin!(server);

    let server_result = tokio::select! {
        result = &mut server => result,
        () = crate::shutdown::signal() => {
            let _ = shutdown_sender.send(true);
            if let Ok(result) = tokio::time::timeout(shutdown_timeout, &mut server).await {
                result
            } else {
                tracing::warn!(?shutdown_timeout, "HTTP graceful-shutdown deadline exceeded");
                Ok(())
            }
        }
    };

    if tokio::time::timeout(shutdown_timeout, database.close())
        .await
        .is_err()
    {
        tracing::warn!(?shutdown_timeout, "database pool close deadline exceeded");
    }
    server_result?;
    Ok(())
}

fn router(state: AppState, server: &ServerSettings, webhook_settings: &WebhookSettings) -> Router {
    let ordinary = ordinary_routes().layer(DefaultBodyLimit::max(server.max_json_body_bytes.get()));
    let ordinary = with_deadline(ordinary, server.request_timeout);

    let upload_control =
        upload_control_routes().layer(DefaultBodyLimit::max(server.max_json_body_bytes.get()));
    let upload_control = with_deadline(upload_control, server.upload_timeout);

    let restore = with_deadline(
        Router::new()
            .route(
                "/api/v1/entries/{entry_id}/versions/{version_id}/restore",
                post(content::restore_version),
            )
            .layer(DefaultBodyLimit::max(server.max_json_body_bytes.get())),
        server.restore_timeout,
    )
    .layer(ConcurrencyLimitLayer::new(
        server.max_concurrent_restores.get(),
    ));

    // One upload route accepts a file of any supported size and decides
    // internally how to store it, so the body limit is the staging ceiling
    // rather than an HTTP one.
    let uploads = with_deadline(
        Router::new()
            .route("/api/v1/uploads", post(content::upload))
            .route(obo::CREATE_FILE_PATH, post(obo::create_file))
            .route(
                delegated_upload::TRANSFER_PATH,
                put(delegated_upload::transfer),
            )
            .layer(DefaultBodyLimit::disable()),
        server.upload_timeout,
    );
    let webhook = with_deadline(
        Router::new()
            .route("/webhook/", post(accounts::webhook))
            .layer(DefaultBodyLimit::max(webhook_settings.max_body_bytes.get())),
        server.request_timeout,
    );

    ordinary
        .merge(upload_control)
        .merge(restore)
        .merge(uploads)
        .merge(webhook)
        .fallback(not_found)
        .layer(ConcurrencyLimitLayer::new(
            server.max_concurrent_requests.get(),
        ))
        .layer(SetSensitiveResponseHeadersLayer::new([header::SET_COOKIE]))
        .layer(SetSensitiveRequestHeadersLayer::new(
            sensitive_request_headers(),
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::contract,
        ))
        .layer(CatchPanicLayer::custom(middleware::handle_panic))
        .layer(axum_middleware::from_fn(middleware::request_scope))
        .with_state(state)
}

fn sharing_routes() -> Router<AppState> {
    Router::new()
        .route(
            invitations::OBO_INVITE_PATH,
            post(invitations::delegated_invite),
        )
        .route(
            invitations::OBO_LINK_PATH,
            post(invitations::delegated_link),
        )
        .route(
            "/api/v1/entries/{entry_id}/invitations",
            get(invitations::list).post(invitations::invite),
        )
        .route(
            "/api/v1/entries/{entry_id}/invitations/{grant_id}",
            delete(invitations::revoke).patch(invitations::change),
        )
        .route(
            "/api/v1/entries/{entry_id}/link-access",
            get(sharing::link_access).put(sharing::set_link_access),
        )
        .route("/api/v1/entries/{entry_id}/logs", get(sharing::logs))
        .route("/api/v1/public/{org_id}/{*path}", get(sharing::public_path))
}

/// The drives a caller opens, what was shared with it, and who may share
/// with a Silicon.
fn drive_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drives", get(drive_handlers::list))
        .route("/api/v1/shared", get(drive_handlers::shared))
        .route(
            "/api/v1/silicons/{silicon}/allowed-accounts",
            get(drive_handlers::allowances),
        )
        .route(
            "/api/v1/silicons/{silicon}/allowed-accounts/{account}",
            put(drive_handlers::allow).delete(drive_handlers::disallow),
        )
}

fn ordinary_routes() -> Router<AppState> {
    Router::new()
        .merge(sharing_routes())
        .merge(drive_routes())
        .route(
            delegated_upload::RESERVE_PATH,
            post(delegated_upload::reserve),
        )
        .route(
            delegated_upload::COMMIT_PATH,
            post(delegated_upload::commit),
        )
        .route(
            delegated_upload::STATUS_PATH,
            post(delegated_upload::status),
        )
        .route(
            delegated_upload::CANCEL_PATH,
            post(delegated_upload::cancel),
        )
        .route(
            delegated::CREATE_FOLDER_PATH,
            post(delegated::create_folder),
        )
        .route(delegated::LIST_ENTRIES_PATH, post(delegated::list_entries))
        .route(delegated::READ_FILE_PATH, post(delegated::read_file))
        .route(delegated::TRASH_ENTRY_PATH, post(delegated::trash_entry))
        .route("/healthz", get(system::health))
        .route("/readyz", get(system::ready))
        .route("/api/version", get(system::version))
        .route("/api/v1/version", get(system::version))
        .route("/api/v1/accounts", get(accounts::discovery))
        .route("/api/v1/me", get(accounts::me))
        .route(
            "/api/v1/entries",
            get(entries::list_entries).post(entries::create_folder),
        )
        .route(
            "/api/v1/entries/{entry_id}",
            get(entries::get_entry)
                .patch(entries::update_entry)
                .delete(entries::delete_entry),
        )
        .route(
            "/api/v1/entries/{entry_id}/permissions",
            get(permissions::list_permissions).post(permissions::grant_permission),
        )
        .route(
            "/api/v1/entries/{entry_id}/permissions/{grant_id}",
            delete(permissions::revoke_permission),
        )
        .route(
            "/api/v1/permissions/effective",
            post(permissions::inspect_permissions),
        )
        .route("/api/v1/search", get(entries::search))
        .route("/api/v1/reports", post(reports::submit))
        .route("/api/v1/telemetry", post(telemetry::submit))
        .route(
            "/api/v1/notifications",
            get(notifications::list_notifications),
        )
        .route(
            "/api/v1/notifications/read",
            post(notifications::read_notifications),
        )
        .route(
            "/api/v1/entries/{entry_id}/content",
            get(content::read_content),
        )
        .route(
            "/api/v1/entries/{entry_id}/download",
            get(content::download_content),
        )
        .route("/org/{org_id}/{*path}", get(entries::resolve_path))
        .route(
            "/api/v1/entries/{entry_id}/activity",
            get(entries::entry_activity),
        )
        .route(
            "/api/v1/entries/{entry_id}/versions",
            get(content::list_versions),
        )
        .route(
            "/api/v1/entries/{entry_id}/self-destruct",
            delete(entries::make_permanent),
        )
        .route("/api/v1/usage", get(usage::organization_usage))
        .route("/api/v1/bin", get(entries::list_bin))
        .route(
            "/api/v1/bin/{entry_id}/restore",
            post(entries::restore_bin_entry),
        )
}

fn upload_control_routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/storage/configuration",
        put(content::configure_storage),
    )
}

fn with_deadline(routes: Router<AppState>, timeout: Duration) -> Router<AppState> {
    routes.layer(axum_middleware::from_fn(move |request, next| async move {
        middleware::enforce_timeout(timeout, request, next).await
    }))
}

fn sensitive_request_headers() -> [HeaderName; 5] {
    [
        header::AUTHORIZATION,
        header::COOKIE,
        HeaderName::from_static("x-accounts-signature"),
        HeaderName::from_static("idempotency-key"),
        HeaderName::from_static("x-briefcase-upload-capability"),
    ]
}

async fn not_found() -> crate::error::AppError {
    crate::error::AppError::NotFound
}

#[cfg(test)]
mod accounts_tests;
#[cfg(test)]
mod drives_tests;
#[cfg(test)]
pub(crate) mod tests;
