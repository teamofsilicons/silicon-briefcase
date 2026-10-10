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
    },
    config::{
        AccountsSettings, ProofIssuers, S3Encryption, S3Settings, ServerSettings,
        WEBHOOK_TOLERANCE, WebhookSettings,
    },
    infrastructure::{
        accounts::{AccountDirectory, AccountsGateway},
        postgres::{PostgresContentRepository, PostgresRepository, accounts::AccountStore},
        s3::S3ObjectStore,
    },
};

use super::{AppState, ContentUseCases, DelegatedUploadUseCases, mapping::ResponseMapper, router};

/// A well-formed (but never valid) Silicon Accounts app secret for tests.
pub(crate) const TEST_APP_SECRET: &str = "sa_app_briefcase_test_secret_0123456789abcdef";
/// A well-formed webhook signing secret for tests.
pub(crate) const TEST_WEBHOOK_SECRET: &str = "whsec_briefcase_test_signing_secret_0123456789";

const CONTRACT: [(&str, &str, &str); 49] = [
    ("/version", "get", "200"),
    ("/accounts", "get", "200"),
    ("/me", "get", "200"),
    ("/entries", "get", "200"),
    ("/entries", "post", "201"),
    ("/entries/{entry_id}", "get", "200"),
    ("/entries/{entry_id}", "patch", "200"),
    ("/entries/{entry_id}", "delete", "204"),
    ("/entries/{entry_id}/content", "get", "200"),
    ("/entries/{entry_id}/download", "get", "200"),
    ("/org/{org_id}/{path}", "get", "200"),
    ("/uploads", "post", "201"),
    ("/obo/files", "post", "410"),
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
    ("/reports", "post", "200"),
    ("/telemetry", "post", "204"),
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
    ("/entries/{entry_id}/invitations", "get", "200"),
    ("/entries/{entry_id}/invitations", "post", "201"),
    (
        "/entries/{entry_id}/invitations/{grant_id}",
        "delete",
        "204",
    ),
    ("/entries/{entry_id}/invitations/{grant_id}", "patch", "200"),
    ("/entries/{entry_id}/self-destruct", "delete", "204"),
    ("/entries/{entry_id}/link-access", "get", "200"),
    ("/entries/{entry_id}/link-access", "put", "200"),
    ("/entries/{entry_id}/logs", "get", "200"),
    ("/public/{org_id}/{path}", "get", "200"),
    ("/obo/invitations", "post", "200"),
    ("/obo/link-access", "post", "200"),
];

#[test]
fn every_openapi_operation_has_the_expected_response_status() -> anyhow::Result<()> {
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
fn openapi_models_accounts_tokens_and_proofs_explicitly() -> anyhow::Result<()> {
    let document: Value = serde_yaml::from_str(include_str!("../../openapi.yaml"))?;
    let global_security = document["security"]
        .as_sequence()
        .ok_or_else(|| anyhow::anyhow!("OpenAPI security must be a sequence"))?;
    // Ordinary operations take only the caller's own access token.
    assert_eq!(global_security.len(), 1);
    assert!(global_security[0]["bearerAuth"].is_sequence());
    assert_eq!(
        global_security[0]
            .as_mapping()
            .map(serde_yaml::Mapping::len),
        Some(1)
    );
    let schemes = &document["components"]["securitySchemes"];
    assert_eq!(schemes["bearerAuth"]["bearerFormat"].as_str(), Some("JWT"));
    assert_eq!(schemes["proofAuth"]["type"].as_str(), Some("http"));
    assert_eq!(schemes["proofAuth"]["scheme"].as_str(), Some("Proof"));
    assert!(schemes["testingEnvironmentKey"].is_null());
    assert!(schemes["oboAccess"].is_null());

    // Delegated operations take only a proof.
    for path in [
        "/obo/uploads/reserve",
        "/obo/uploads/commit",
        "/obo/uploads/status",
        "/obo/uploads/cancel",
        "/obo/folders/create",
        "/obo/entries/list",
        "/obo/files/read",
        "/obo/entries/trash",
        "/obo/invitations",
        "/obo/link-access",
    ] {
        let security = document["paths"][path]["post"]["security"]
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("{path} security must be a sequence"))?;
        assert_eq!(security.len(), 1, "{path}");
        assert!(security[0]["proofAuth"].is_sequence(), "{path}");
        assert_eq!(
            security[0].as_mapping().map(serde_yaml::Mapping::len),
            Some(1),
            "{path}"
        );
    }

    // Only the capability transfer still names a tenant selector header;
    // every other operation derives the tenant from the verified credential.
    let text = include_str!("../../openapi.yaml");
    assert_eq!(
        text.matches("X-Org-ID").count(),
        2,
        "parameter name and transfer prose"
    );
    assert_eq!(
        text.matches("#/components/parameters/UploadTenant").count(),
        1,
        "only transferDelegatedUpload selects a tenant by header"
    );
    assert!(!text.contains("X-Briefcase-App-Secret"));
    assert!(!text.contains("X-IAM-OBO-Access-Token"));
    Ok(())
}

#[tokio::test]
async fn retired_routes_are_not_registered() -> anyhow::Result<()> {
    let application = test_router()?;
    let identifier = Uuid::now_v7();
    for (method, path) in [
        (
            Method::POST,
            format!("/api/v1/entries/{identifier}/access-requests"),
        ),
        (Method::POST, "/api/v1/access-requests".to_owned()),
        (
            Method::POST,
            format!("/api/v1/access-requests/{identifier}/decision"),
        ),
        (Method::GET, "/api/v1/iam".to_owned()),
        (Method::GET, "/api/v1/auth/status".to_owned()),
        (Method::POST, "/api/v1/auth/slt".to_owned()),
        (Method::POST, "/api/v1/auth/refresh".to_owned()),
        (
            Method::GET,
            "/api/v1/organizations/tos/testing-environments".to_owned(),
        ),
        (Method::GET, "/api/v1/testing-environment".to_owned()),
        (
            Method::PUT,
            format!(
                "/internal/honeycomb/organizations/tos/testing-environments/{identifier}/operations/{identifier}"
            ),
        ),
    ] {
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
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
            .replace("{org_id}", "zQo")
            .replace("{upload_id}", &identifier)
            .replace("{version_id}", &identifier);
        let path = match path.as_str() {
            "/search" => format!("/api/v1{path}?q=registered"),
            // The permanent URL is served outside the versioned API base.
            "/org/zQo/{path}" => "/org/zQo/private/zQo/notes.md".to_owned(),
            _ => format!("/api/v1{path}"),
        };
        let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes())?;
        let request = Request::builder()
            .method(method.clone())
            .uri(&path)
            .header("x-request-id", "route-contract-test");
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
async fn accounts_discovery_is_public_and_me_needs_an_access_token() -> anyhow::Result<()> {
    let application = test_router()?;
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/accounts")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let bytes = axum::body::to_bytes(response.into_body(), 8192).await?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)?;
    assert_eq!(body["app_id"], "briefcase");
    assert_eq!(body["accounts_url"], "http://127.0.0.1:9");
    assert_eq!(
        body["delegated_scopes"].as_array().map(Vec::len),
        Some(crate::domain::delegation::DELEGATED_SCOPES.len())
    );

    for (authorization, code) in [
        (None, "authentication_required"),
        (Some("Bearer not-a-jwt"), "token_malformed"),
        (Some("Proof sap_abcdef"), "proof_not_accepted_here"),
    ] {
        let mut request = Request::builder().uri("/api/v1/me");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = application
            .clone()
            .oneshot(request.body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = axum::body::to_bytes(response.into_body(), 8192).await?;
        let body: serde_json::Value = serde_json::from_slice(&bytes)?;
        assert_eq!(body["error"]["code"], code);
        assert!(body["error"]["message"].as_str().is_some_and(|message| {
            message.contains("Authorization") || message.contains("access token")
        }));
    }
    Ok(())
}

#[tokio::test]
async fn telemetry_relay_is_anonymous_and_rejects_arbitrary_payloads() -> anyhow::Result<()> {
    use briefcase_client::telemetry::{Event, Source, Stage};
    let router = test_router()?;
    let mut event = serde_json::to_value(Event::new(Source::Cli, "upload", Stage::Completed))?;
    for (mutation, expected) in [(None, 204), (Some("token"), 422), (Some("source"), 422)] {
        if let Some(field) = mutation {
            event[field] = serde_json::json!(if field == "source" {
                "backend"
            } else {
                "slt_private"
            });
        }
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/telemetry")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&event)?))?,
            )
            .await?;
        assert_eq!(response.status().as_u16(), expected);
        event
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("object"))?
            .remove("token");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "explicit live Space Station check; requires the private collector configuration"]
async fn telemetry_live_delivery() -> anyhow::Result<()> {
    use briefcase_client::telemetry::{Event, Source, Stage};
    crate::telemetry::init_process(
        crate::config::RuntimeEnvironment::Test,
        "silicon_briefcase=info",
    )?;
    anyhow::ensure!(
        crate::telemetry::configured(),
        "collector is not configured"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/api/v1/", listener.local_addr()?);
    let router = test_router()?;
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    for source in [Source::Sdk, Source::Cli, Source::Daemon, Source::Web] {
        let mut event = Event::new(source, "telemetry_integration_check", Stage::Completed);
        event.testing = true;
        event.count = Some(1);
        briefcase_client::telemetry::submit(&base, &event).await?;
        println!("live telemetry event {} source {source:?}", event.id);
    }
    let mut acked = false;
    for _ in 0..40 {
        if crate::telemetry::flush() {
            acked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    server.abort();
    anyhow::ensure!(
        acked,
        "Space Station did not acknowledge within the live-check window"
    );
    Ok(())
}

fn test_router() -> anyhow::Result<Router> {
    let database = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(10))
        .connect_lazy("postgresql://briefcase:briefcase@127.0.0.1:9/briefcase")?;
    let (state, server, webhook) = test_state(database)?;
    Ok(router(state, &server, &webhook))
}

/// Silicon Accounts settings pointing at `accounts_url` with test credentials.
pub(crate) fn test_accounts_settings(
    accounts_url: &str,
    proof_issuers: &str,
) -> anyhow::Result<AccountsSettings> {
    let url = Url::parse(accounts_url)?;
    Ok(AccountsSettings {
        public_url: url.clone(),
        issuer: accounts_url.trim_end_matches('/').to_owned(),
        api_url: url,
        app_id: "briefcase".to_owned(),
        app_secret: SecretString::from(TEST_APP_SECRET.to_owned()),
        request_timeout: Duration::from_secs(2),
        account_cache_ttl: Duration::from_secs(300),
        proof_issuers: ProofIssuers::parse(proof_issuers)
            .map_err(|reason| anyhow::anyhow!(reason))?,
    })
}

pub(crate) fn test_state(
    database: sqlx::PgPool,
) -> anyhow::Result<(AppState, ServerSettings, WebhookSettings)> {
    let accounts = test_accounts_settings("http://127.0.0.1:9", "")?;
    test_state_with_accounts(database, &accounts, None)
}

/// Builds the API state against a database and a (stub) Silicon Accounts.
pub(crate) fn test_state_with_accounts(
    database: sqlx::PgPool,
    accounts: &AccountsSettings,
    jwks: Option<silicon_accounts_client::Jwks>,
) -> anyhow::Result<(AppState, ServerSettings, WebhookSettings)> {
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
    let concrete_content = Arc::new(PostgresContentRepository::new(repository, s3.clone()));
    let content_repository: Arc<dyn ContentRepository> = concrete_content.clone();
    let delegated_repository: Arc<dyn DelegatedUploadRepository> = concrete_content.clone();
    // A behavior version and a real clock, as a loaded production
    // configuration has: building the role-assuming client for a
    // bring-your-own bucket needs both. There are no credentials, so nothing
    // ever reaches AWS.
    let object_store: Arc<dyn ObjectStore> = Arc::new(S3ObjectStore::new(
        aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .time_source(aws_smithy_async::time::SystemTimeSource::new())
            .build(),
        None,
        false,
    ));
    let content: Arc<ContentUseCases> = Arc::new(ContentService::new(
        content_repository,
        Arc::clone(&object_store),
        s3.temporary_directory.clone(),
    ));
    let delegated_uploads: Arc<DelegatedUploadUseCases> = Arc::new(DelegatedUploadService::new(
        delegated_repository,
        Arc::clone(&object_store),
        122,
        s3.operation_timeout,
    ));
    let mut gateway = AccountsGateway::new(accounts)?;
    if let Some(jwks) = jwks {
        gateway = gateway.with_jwks(jwks);
    }
    let directory = AccountDirectory::new(
        Arc::new(gateway),
        AccountStore::new(database.clone()),
        accounts,
    );
    let webhook = WebhookSettings {
        signing_secret: SecretString::from(TEST_WEBHOOK_SECRET.to_owned()),
        previous_signing_secret: None,
        tolerance: WEBHOOK_TOLERANCE,
        max_body_bytes: nonzero(262_144)?,
    };
    let server = ServerSettings {
        bind_addr: ([127, 0, 0, 1], 0).into(),
        public_base_url: Url::parse("https://briefcase.example/api/v1/")?,
        public_site_base_url: Url::parse("https://briefcase.example/")?,
        request_timeout: Duration::from_secs(5),
        upload_timeout: Duration::from_secs(5),
        restore_timeout: Duration::from_secs(5),
        max_json_body_bytes: nonzero(1_048_576)?,
        max_concurrent_requests: nonzero(32)?,
        max_concurrent_restores: nonzero(2)?,
        shutdown_timeout: Duration::from_secs(1),
    };
    let state = AppState {
        accounts: Arc::new(directory),
        metadata: MetadataService::new(metadata_repository),
        content,
        content_adapter: concrete_content,
        objects: object_store,
        delegated_uploads,
        database,
        contract_database: None,
        mapper: ResponseMapper::new(&server.public_base_url, server.public_site_base_url.clone()),
        temporary_directory: s3.temporary_directory,
        webhook_settings: webhook.clone(),
    };
    Ok((state, server, webhook))
}

fn nonzero(value: usize) -> anyhow::Result<NonZeroUsize> {
    NonZeroUsize::new(value).ok_or_else(|| anyhow::anyhow!("test fixture must be non-zero"))
}
