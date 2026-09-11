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
        webhook::IamWebhookRepository,
    },
    config::{ServerSettings, Settings, WebhookSettings},
    infrastructure::{
        iam::IamClient,
        postgres::{self, PostgresContentRepository, PostgresRepository},
        s3::S3ObjectStore,
        testing::TestingEnvironmentStore,
    },
};

mod auth;
pub mod cursor;
mod delivery;
pub mod dto;
mod extract;
mod handlers;
mod mapping;
mod middleware;
mod state;
pub mod upload;
pub mod validation;
pub mod versioning;
mod webhook;

use handlers::{
    content, delegated, delegated_upload, entries, notifications, obo, permissions, session,
    system, testing, usage,
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
    let (test_database, testing) = if let Some(testing) = &settings.testing {
        let test_database =
            postgres::connect(&testing.database, "briefcase-api-test-plane").await?;
        postgres::verify_tenant_isolated_role(&test_database).await?;
        postgres::verify_distinct_databases(&database, &test_database).await?;
        let store = TestingEnvironmentStore::new(
            database.clone(),
            test_database.clone(),
            &testing.encryption_key,
        )?;
        (Some(test_database), Some(Arc::new(store)))
    } else {
        (None, None)
    };
    let repository = test_database.as_ref().map_or_else(
        || PostgresRepository::new(database.clone()),
        |test| PostgresRepository::new(database.clone()).with_test_pool(test.clone()),
    );
    let metadata_repository: Arc<dyn MetadataRepository> = Arc::new(repository.clone());
    let webhook_repository: Arc<dyn IamWebhookRepository> = Arc::new(repository.clone());
    let concrete_content = Arc::new(PostgresContentRepository::new(
        repository,
        settings.s3.clone(),
    ));
    let content_repository: Arc<dyn ContentRepository> = concrete_content.clone();
    let delegated_repository: Arc<dyn DelegatedUploadRepository> = concrete_content;
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
        object_store,
        settings.s3.temporary_directory.clone(),
    ));
    let state = AppState {
        iam: Arc::new(IamClient::connect(&settings.iam).await?),
        metadata: MetadataService::new(metadata_repository),
        content,
        delegated_uploads,
        webhook_repository,
        database: database.clone(),
        mapper: mapping::ResponseMapper::new(
            &settings.server.public_base_url,
            settings.server.public_site_base_url.clone(),
        ),
        temporary_directory: settings.s3.temporary_directory.clone(),
        webhook_settings: settings.webhook.clone(),
        testing,
    };
    let application = router(state, &settings.server, &settings.webhook);
    let listener = tokio::net::TcpListener::bind(settings.server.bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "Briefcase API listening");

    serve_until_shutdown(
        listener,
        application,
        database,
        test_database,
        settings.server.shutdown_timeout,
    )
    .await
}

async fn serve_until_shutdown(
    listener: tokio::net::TcpListener,
    application: Router,
    database: sqlx::PgPool,
    test_database: Option<sqlx::PgPool>,
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
    if let Some(test_database) = test_database
        && tokio::time::timeout(shutdown_timeout, test_database.close())
            .await
            .is_err()
    {
        tracing::warn!(
            ?shutdown_timeout,
            "test database pool close deadline exceeded"
        );
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
            .route("/webhook/", post(system::iam_webhook))
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
        .layer(CatchPanicLayer::custom(middleware::handle_panic))
        .layer(axum_middleware::from_fn(middleware::request_scope))
        .with_state(state)
}

fn ordinary_routes() -> Router<AppState> {
    Router::new()
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
        .route("/api/v1/iam", get(session::iam_info))
        .route("/api/v1/auth/status", get(session::status))
        .route("/api/v1/auth/slt", post(session::exchange_slt))
        .route("/api/v1/auth/refresh", post(session::refresh))
        .merge(testing_environment_routes())
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
        .route("/api/v1/usage", get(usage::organization_usage))
        .route("/api/v1/bin", get(entries::list_bin))
        .route(
            "/api/v1/bin/{entry_id}/restore",
            post(entries::restore_bin_entry),
        )
}

fn testing_environment_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{org_id}/testing-environments",
            get(testing::list).post(testing::create),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}",
            get(testing::get)
                .patch(testing::update)
                .delete(testing::delete),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}/key",
            get(testing::key),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}/key-rotations",
            post(testing::rotate_key),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}/iam-pairings",
            post(testing::replace_iam_pairing),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}/cleanings",
            post(testing::clean),
        )
        .route(
            "/api/v1/organizations/{org_id}/testing-environments/{environment_id}/restorations",
            post(testing::restore),
        )
        .route("/api/v1/testing-environment", get(testing::current))
        .route(
            "/api/v1/testing-environment/cleanings",
            post(testing::clean_current),
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

fn sensitive_request_headers() -> [HeaderName; 7] {
    [
        header::AUTHORIZATION,
        header::COOKIE,
        HeaderName::from_static("x-iam-obo-access-proof"),
        HeaderName::from_static("x-silicon-iam-signature"),
        HeaderName::from_static("idempotency-key"),
        HeaderName::from_static("x-testing-environment-key"),
        HeaderName::from_static("x-briefcase-upload-capability"),
    ]
}

async fn not_found() -> crate::error::AppError {
    crate::error::AppError::NotFound
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, sync::Arc, time::Duration};

    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use secrecy::SecretString;
    use serde_yaml::Value;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt as _;
    use url::Url;
    use uuid::Uuid;

    use crate::{
        application::{
            content::{ContentRepository, ContentService},
            delegated_upload::{DelegatedUploadRepository, DelegatedUploadService},
            ports::ObjectStore,
            service::{MetadataRepository, MetadataService},
            webhook::IamWebhookRepository,
        },
        config::{IamSettings, S3Encryption, S3Settings, ServerSettings, WebhookSettings},
        infrastructure::{
            iam::IamClient,
            postgres::{PostgresContentRepository, PostgresRepository},
            s3::S3ObjectStore,
        },
    };

    use super::{
        AppState, ContentUseCases, DelegatedUploadUseCases, mapping::ResponseMapper, router,
    };

    const CONTRACT: [(&str, &str, &str); 50] = [
        ("/version", "get", "200"),
        ("/iam", "get", "200"),
        ("/auth/status", "get", "200"),
        ("/auth/slt", "post", "200"),
        ("/auth/refresh", "post", "200"),
        ("/organizations/{org_id}/testing-environments", "get", "200"),
        (
            "/organizations/{org_id}/testing-environments",
            "post",
            "201",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}",
            "get",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}",
            "patch",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}",
            "delete",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}/key",
            "get",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}/key-rotations",
            "post",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}/iam-pairings",
            "post",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}/cleanings",
            "post",
            "200",
        ),
        (
            "/organizations/{org_id}/testing-environments/{environment_id}/restorations",
            "post",
            "200",
        ),
        ("/testing-environment", "get", "200"),
        ("/testing-environment/cleanings", "post", "200"),
        ("/entries", "get", "200"),
        ("/entries", "post", "201"),
        ("/entries/{entry_id}", "get", "200"),
        ("/entries/{entry_id}", "patch", "200"),
        ("/entries/{entry_id}", "delete", "204"),
        ("/entries/{entry_id}/content", "get", "200"),
        ("/entries/{entry_id}/download", "get", "200"),
        ("/org/{org_id}/{path}", "get", "200"),
        ("/uploads", "post", "201"),
        ("/obo/files", "post", "201"),
        ("/obo/uploads/reserve", "post", "200"),
        ("/obo/uploads/commit", "post", "200"),
        ("/obo/uploads/status", "post", "200"),
        ("/obo/uploads/cancel", "post", "200"),
        ("/obo/uploads/{upload_id}/content", "put", "200"),
        ("/obo/folders/create", "post", "201"),
        ("/obo/entries/list", "post", "200"),
        ("/obo/files/read", "post", "200"),
        ("/obo/entries/trash", "post", "204"),
        ("/entries/{entry_id}/permissions", "get", "200"),
        ("/entries/{entry_id}/permissions", "post", "201"),
        (
            "/entries/{entry_id}/permissions/{grant_id}",
            "delete",
            "204",
        ),
        ("/permissions/effective", "post", "200"),
        ("/search", "get", "200"),
        ("/usage", "get", "200"),
        ("/notifications", "get", "200"),
        ("/notifications/read", "post", "200"),
        ("/entries/{entry_id}/activity", "get", "200"),
        ("/entries/{entry_id}/versions", "get", "200"),
        (
            "/entries/{entry_id}/versions/{version_id}/restore",
            "post",
            "200",
        ),
        ("/bin", "get", "200"),
        ("/bin/{entry_id}/restore", "post", "200"),
        ("/storage/configuration", "put", "200"),
    ];

    #[test]
    fn every_openapi_operation_has_the_expected_success_status() -> anyhow::Result<()> {
        let document: Value = serde_yaml::from_str(include_str!("../../openapi.yaml"))?;
        let mut documented_operations = 0_usize;
        for path_item in document["paths"]
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("OpenAPI paths must be a mapping"))?
            .values()
        {
            let Some(methods) = path_item.as_mapping() else {
                continue;
            };
            documented_operations += methods
                .keys()
                .filter_map(Value::as_str)
                .filter(|method| matches!(*method, "get" | "post" | "put" | "patch" | "delete"))
                .count();
        }
        assert_eq!(documented_operations, CONTRACT.len());

        for (path, method, success_status) in CONTRACT {
            let responses = &document["paths"][path][method]["responses"];
            assert!(
                responses[success_status].is_mapping(),
                "missing {success_status} response for {method} {path}"
            );
        }
        Ok(())
    }

    #[test]
    fn openapi_models_production_and_test_plane_credentials_explicitly() -> anyhow::Result<()> {
        let document: Value = serde_yaml::from_str(include_str!("../../openapi.yaml"))?;
        let global_security = document["security"]
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("OpenAPI security must be a sequence"))?;

        // Ordinary operations accept a production bearer by itself or the
        // same IAM actor credential plus the Briefcase root that selects a
        // test plane. The key never replaces actor authentication.
        assert_eq!(global_security.len(), 2);
        assert!(global_security[0]["bearerAuth"].is_sequence());
        assert_eq!(
            global_security[0]
                .as_mapping()
                .map(serde_yaml::Mapping::len),
            Some(1)
        );
        assert!(global_security[1]["bearerAuth"].is_sequence());
        assert!(global_security[1]["testingEnvironmentKey"].is_sequence());
        assert_eq!(
            global_security[1]
                .as_mapping()
                .map(serde_yaml::Mapping::len),
            Some(2)
        );

        // The application endpoint likewise keeps the IAM proof and app ID
        // together, with the root key added only for its test-plane form.
        let obo_security = document["paths"]["/obo/files"]["post"]["security"]
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("OBO security must be a sequence"))?;
        assert_eq!(obo_security.len(), 2);
        assert!(obo_security[0]["oboAccess"].is_sequence());
        assert!(obo_security[0]["appId"].is_sequence());
        assert_eq!(
            obo_security[0].as_mapping().map(serde_yaml::Mapping::len),
            Some(2)
        );
        assert!(obo_security[1]["oboAccess"].is_sequence());
        assert!(obo_security[1]["appId"].is_sequence());
        assert!(obo_security[1]["testingEnvironmentKey"].is_sequence());
        assert_eq!(
            obo_security[1].as_mapping().map(serde_yaml::Mapping::len),
            Some(3)
        );

        let storage_security = document["paths"]["/storage/configuration"]["put"]["security"]
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("storage security must be a sequence"))?;
        assert_eq!(storage_security.len(), 2);
        assert!(storage_security[0]["bearerAuth"].is_sequence());
        assert_eq!(
            storage_security[0]
                .as_mapping()
                .map(serde_yaml::Mapping::len),
            Some(1)
        );
        assert!(storage_security[1]["bearerAuth"].is_sequence());
        assert!(storage_security[1]["testingEnvironmentKey"].is_sequence());
        assert_eq!(
            storage_security[1]
                .as_mapping()
                .map(serde_yaml::Mapping::len),
            Some(2)
        );
        Ok(())
    }

    #[tokio::test]
    async fn retired_access_request_routes_are_not_registered() -> anyhow::Result<()> {
        let application = test_router()?;
        let identifier = Uuid::now_v7();
        for path in [
            format!("/api/v1/entries/{identifier}/access-requests"),
            "/api/v1/access-requests".to_owned(),
            format!("/api/v1/access-requests/{identifier}/decision"),
        ] {
            let response = application
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(&path)
                        .body(Body::empty())?,
                )
                .await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn every_contracted_method_is_registered_by_the_runtime_router() -> anyhow::Result<()> {
        let application = test_router()?;
        let identifier = Uuid::now_v7().to_string();

        for (path, method, _) in CONTRACT {
            let path = path
                .replace("{entry_id}", &identifier)
                .replace("{grant_id}", &identifier)
                .replace("{environment_id}", &identifier)
                .replace("{org_id}", "tos")
                .replace("{upload_id}", &identifier)
                .replace("{version_id}", &identifier)
                .replace("{part_number}", "1");
            let path = match path.as_str() {
                "/search" => format!("/api/v1{path}?q=registered"),
                // The permanent URL is served outside the versioned API base.
                "/org/tos/{path}" => "/org/tos/private/cos:tos/notes.md".to_owned(),
                _ => format!("/api/v1{path}"),
            };
            let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes())?;
            let mut request = Request::builder()
                .method(method.clone())
                .uri(&path)
                .header("x-request-id", "route-contract-test");
            if method == Method::PUT && path.contains("/parts/") {
                request = request.header(header::CONTENT_TYPE, "application/octet-stream");
            }
            let response = application
                .clone()
                .oneshot(request.body(Body::empty())?)
                .await?;

            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "runtime did not register {method} {path}"
            );
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "runtime did not bind {method} to {path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some("route-contract-test")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn system_and_fallback_routes_are_exposed_through_middleware() -> anyhow::Result<()> {
        let application = test_router()?;
        for path in ["/healthz", "/api/v1/version"] {
            let response = application
                .clone()
                .oneshot(Request::get(path).body(Body::empty())?)
                .await?;
            assert_eq!(response.status(), StatusCode::OK, "GET {path}");
            assert!(response.headers().contains_key("x-request-id"));
        }

        let response = application
            .oneshot(Request::get("/not-a-route").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        Ok(())
    }

    #[tokio::test]
    async fn iam_discovery_and_logged_out_status_are_public_and_not_cached() -> anyhow::Result<()> {
        let application = test_router()?;
        for path in ["/api/v1/iam", "/api/v1/auth/status"] {
            let response = application
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty())?)
                .await?;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("no-store")
            );
            let bytes = axum::body::to_bytes(response.into_body(), 4096).await?;
            let body: serde_json::Value = serde_json::from_slice(&bytes)?;
            if path.ends_with("/iam") {
                assert!(body["app_id"].as_str().is_some_and(|id| id.contains('>')));
                assert!(body["test_environment_id"].is_null());
                assert_eq!(body.as_object().map(serde_json::Map::len), Some(3));
            } else {
                assert_eq!(body["authenticated"], false);
                assert!(body["actor"].is_null());
            }
        }
        Ok(())
    }

    fn test_router() -> anyhow::Result<Router> {
        let database = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(10))
            .connect_lazy("postgresql://briefcase:briefcase@127.0.0.1:9/briefcase")?;
        let repository = PostgresRepository::new(database.clone());
        let s3 = S3Settings {
            region: "ap-south-1".to_owned(),
            bucket: "briefcase-test".to_owned(),
            key_prefix: "organizations".to_owned(),
            endpoint_url: None,
            force_path_style: false,
            encryption: S3Encryption::SseS3,
            temporary_directory: std::env::temp_dir().join("silicon-briefcase-tests"),
            operation_timeout: Duration::from_secs(1),
        };
        let metadata_repository: Arc<dyn MetadataRepository> = Arc::new(repository.clone());
        let webhook_repository: Arc<dyn IamWebhookRepository> = Arc::new(repository.clone());
        let concrete_content = Arc::new(PostgresContentRepository::new(repository, s3.clone()));
        let content_repository: Arc<dyn ContentRepository> = concrete_content.clone();
        let delegated_repository: Arc<dyn DelegatedUploadRepository> = concrete_content;
        let object_store: Arc<dyn ObjectStore> = Arc::new(S3ObjectStore::new(
            aws_config::SdkConfig::builder().build(),
            None,
            false,
        ));
        let content: Arc<ContentUseCases> = Arc::new(ContentService::new(
            content_repository,
            Arc::clone(&object_store),
            s3.temporary_directory.clone(),
        ));
        let delegated_uploads: Arc<DelegatedUploadUseCases> =
            Arc::new(DelegatedUploadService::new(
                delegated_repository,
                object_store,
                122,
                s3.operation_timeout,
            ));
        let iam_base = Url::parse("http://127.0.0.1:9/")?;
        let iam = IamClient::new_without_handshake(&IamSettings {
            base_url: iam_base,
            app_id: "tos>briefcase".to_owned(),
            app_secret: SecretString::from(
                "ask_0123456789012345678901234567890123456789012".to_owned(),
            ),
            request_timeout: Duration::from_millis(10),
            max_response_bytes: nonzero(1_048_576)?,
        })?;
        let webhook = WebhookSettings {
            signing_secrets: std::collections::BTreeMap::from([(
                1,
                SecretString::from("01234567890123456789012345678901".to_owned()),
            )]),
            replay_window: Duration::from_secs(300),
            max_body_bytes: nonzero(262_144)?,
        };
        let server = ServerSettings {
            bind_addr: ([127, 0, 0, 1], 0).into(),
            public_base_url: Url::parse("https://briefcase.example/api/v1/")?,
            public_site_base_url: Url::parse("https://briefcase.example/")?,
            request_timeout: Duration::from_secs(1),
            upload_timeout: Duration::from_secs(1),
            restore_timeout: Duration::from_secs(1),
            max_json_body_bytes: nonzero(1_048_576)?,
            max_concurrent_requests: nonzero(32)?,
            max_concurrent_restores: nonzero(2)?,
            shutdown_timeout: Duration::from_secs(1),
        };
        let state = AppState {
            iam: Arc::new(iam),
            metadata: MetadataService::new(metadata_repository),
            content,
            delegated_uploads,
            webhook_repository,
            database,
            mapper: ResponseMapper::new(
                &server.public_base_url,
                server.public_site_base_url.clone(),
            ),
            temporary_directory: s3.temporary_directory,
            webhook_settings: webhook.clone(),
            testing: None,
        };
        Ok(router(state, &server, &webhook))
    }

    fn nonzero(value: usize) -> anyhow::Result<NonZeroUsize> {
        NonZeroUsize::new(value).ok_or_else(|| anyhow::anyhow!("test fixture must be non-zero"))
    }
}
