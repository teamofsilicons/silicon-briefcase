//! Real PostgreSQL/RLS and fake-IAM regressions for imported multi-org worlds.
use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use axum::{body::Bytes, extract::State, http::HeaderMap};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

use super::{auth::IamAction, extract, handlers, state::AppState};
use crate::{
    application::context::ExecutionContext,
    config::IamSettings,
    domain::entry::EntryPath,
    error::AppError,
    infrastructure::{iam::IamClient, postgres, testing::TestingEnvironmentStore},
    request_context,
};

const APP: &str = "briefcase";
const ISSUER: &str = "waveform";
const TOKEN: &str = "oat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROOF: &str = "obo_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ENDPOINT: &str = "briefcase.folders.create";

struct Fixture {
    state: AppState,
    iam: MockServer,
    control: PgPool,
    data: PgPool,
    org: String,
    owner: String,
    environment: Uuid,
    principal: Uuid,
    membership: Uuid,
    organization: Uuid,
    secret: SecretString,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let (Ok(control_url), Ok(data_url)) = (
            std::env::var("BRIEFCASE_TEST_CONTROL_DATABASE_URL"),
            std::env::var("BRIEFCASE_TEST_DATA_DATABASE_URL"),
        ) else {
            return Ok(None);
        };
        let control = PgPool::connect_with(
            control_url
                .parse::<sqlx::postgres::PgConnectOptions>()?
                .options([("search_path", "public")]),
        )
        .await?;
        let data = PgPool::connect_with(
            data_url
                .parse::<sqlx::postgres::PgConnectOptions>()?
                .options([("search_path", "public")]),
        )
        .await?;
        postgres::migrate(&control).await?;
        postgres::migrate(&data).await?;
        let runtime_control = restricted_pool(&control_url).await?;
        let runtime_data = restricted_pool(&data_url).await?;
        postgres::verify_tenant_isolated_role(&runtime_control).await?;
        postgres::verify_tenant_isolated_role(&runtime_data).await?;
        let (mut state, _, _) = super::tests::test_state_with_test_database(
            runtime_control.clone(),
            Some(runtime_data.clone()),
        )?;
        let iam = MockServer::start().await;
        state.iam = Arc::new(IamClient::new_without_handshake(&IamSettings {
            base_url: iam.uri().parse()?,
            app_id: APP.to_owned(),
            app_secret: SecretString::from(format!("ask_{}", "p".repeat(43))),
            request_timeout: Duration::from_secs(2),
            max_response_bytes: NonZeroUsize::new(1_048_576)
                .ok_or_else(|| anyhow::anyhow!("size"))?,
        })?);
        state.testing = Some(Arc::new(TestingEnvironmentStore::new(
            runtime_control,
            runtime_data,
            &SecretString::from("MDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDA="),
            "briefcase",
        )?));
        let fixture = Self {
            state,
            iam,
            control,
            data,
            org: format!("test-{}", Uuid::new_v4().simple()),
            owner: format!("owner-{}", Uuid::new_v4().simple()),
            environment: Uuid::new_v4(),
            principal: Uuid::new_v4(),
            membership: Uuid::new_v4(),
            organization: Uuid::new_v4(),
            secret: SecretString::from(format!("ask_{}xxxxxxxxxxx", Uuid::new_v4().simple())),
        };
        fixture.discovery(1, None).await;
        Ok(Some(fixture))
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.iam.reset().await;
        self.discovery(100, Some("2026-09-14T00:00:00Z")).await;
        extract::optional_testing_access(&self.state, &self.headers(false)?).await?;
        sqlx::query("DELETE FROM briefcase.testing_environments WHERE environment_id=$1")
            .bind(self.environment)
            .execute(&self.control)
            .await?;
        Ok(())
    }

    fn basic(&self) -> String {
        format!(
            "Basic {}",
            STANDARD.encode(format!("{APP}:{}", self.secret.expose_secret()))
        )
    }

    async fn discovery(&self, version: i64, cleaned: Option<&str>) {
        Mock::given(method("GET"))
            .and(path("/api/v1/application/testing-context"))
            .and(header("authorization", self.basic()))
            .and(header("x-testing-application", self.basic()))
            .respond_with(response(json!({
                "environment_id": self.environment,
                "environment": {"environment_id": self.environment, "org_id": self.owner,
                    "name":"Imported world", "description":null, "version":version,
                    "key_generation":1, "cleaned_at":cleaned, "created_at":"2026-09-13T00:00:00Z",
                    "creator_type":"carbon", "creator_id":"production-owner"},
                "application":{"app_id":APP,"base_url":"https://briefcase.example.test",
                    "app_scope":{"iam":[],"external":[]},"webhook_scope":[],"testing_idle_days":30}
            })))
            .mount(&self.iam)
            .await;
    }

    fn snapshot(&self) -> Value {
        json!({"principal_id":self.principal,"actor_type":"carbon","public_id":"test-carbon",
            "organization_id":self.organization,"org_id":self.org,
            "membership_id":self.membership,"membership_version":7,"authorization_epoch":7,
            "audience":APP,"testing_environment_id":self.environment,
            "scopes":["self.identity.read","self.membership.read","self.tags.read"],
            "org_role":"owner","tags":[]})
    }

    fn introspection(&self) -> Value {
        json!({"active":true,"principal_id":self.principal,"actor_type":"carbon","client_id":APP,
            "org_id":self.org,"membership_id":self.membership,
            "session_id":"01990a9d-86f1-7000-8000-000000000003",
            "scope":"self.identity.read self.membership.read self.tags.read","audience":APP,
            "authorization":self.snapshot(),"authorization_epoch":7,
            "issued_at":1_700_000_000_i64,"expires_at":4_070_908_800_i64})
    }

    async fn introspect(&self, body: Value) {
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .and(header("authorization", self.basic()))
            .and(header("x-testing-application", self.basic()))
            .and(header("x-org-id", self.org.as_str()))
            .respond_with(response(body))
            .mount(&self.iam)
            .await;
    }

    fn headers(&self, delegated: bool) -> anyhow::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert("x-org-id", self.org.parse()?);
        headers.insert(
            "x-briefcase-app-secret",
            self.secret.expose_secret().parse()?,
        );
        if delegated {
            headers.insert("x-app-id", ISSUER.parse()?);
            headers.insert("x-iam-obo-access-proof", PROOF.parse()?);
        } else {
            headers.insert("authorization", format!("Bearer {TOKEN}").parse()?);
        }
        Ok(headers)
    }

    async fn authenticate(&self, bearer_only: bool) -> Result<ExecutionContext, AppError> {
        let headers = self.headers(false).map_err(|_| AppError::NotFound)?;
        test_scope("imported-world-auth".into(), async {
            if bearer_only {
                extract::authenticate_bearer(&self.state, &headers, IamAction::ListEntries).await
            } else {
                extract::authenticate(&self.state, &headers, IamAction::ListEntries, &self.org)
                    .await
            }
        })
        .await
    }

    fn proof_response(&self) -> Value {
        let mut snapshot = self.snapshot();
        snapshot["scopes"] = json!([
            format!("obo:{APP}:{ENDPOINT}"),
            "self.identity.read",
            "self.membership.read",
            "self.tags.read"
        ]);
        json!({"valid":true,"proof_id":Uuid::new_v4(),"issuer_app_id":ISSUER,"audience":APP,
            "authorization":snapshot,"actor":{"principal_id":self.principal,"type":"carbon","public_id":"test-carbon"},
            "org_id":self.org,"endpoint":{"endpoint_id":ENDPOINT,"path":handlers::delegated::CREATE_FOLDER_PATH},
            "metadata":{},"expires_at":"2099-01-01T00:00:00Z","consumed_at":"2026-09-14T00:00:00Z"})
    }

    async fn proof(&self, body: &[u8], response_body: Value) {
        Mock::given(method("POST")).and(path("/api/v1/obo-access/verify"))
            .and(header("authorization", self.basic()))
            .and(header("x-testing-application", self.basic()))
            .and(body_json(json!({"access_proof":PROOF,"request":{"method":"POST",
                "path":handlers::delegated::CREATE_FOLDER_PATH,"body_sha256":hex::encode(Sha256::digest(body))}})))
            .respond_with(response(response_body)).mount(&self.iam).await;
    }
}

fn test_scope<T>(
    request_id: String,
    future: impl std::future::Future<Output = T>,
) -> impl std::future::Future<Output = T> {
    Box::pin(request_context::scope(request_id, future))
}

fn response(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("cache-control", "no-store")
        .insert_header("pragma", "no-cache")
        .set_body_json(body)
}

async fn restricted_pool(url: &str) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE briefcase_api")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await?)
}

#[tokio::test]
async fn imported_world_authorizes_data_org_from_live_iam_not_control_owner() -> anyhow::Result<()>
{
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    f.introspect(f.introspection()).await;
    for bearer_only in [false, true] {
        let context = f.authenticate(bearer_only).await?;
        assert_eq!(context.authorization().organization_id().as_str(), f.org);
        assert_eq!(
            context
                .testing_environment()
                .map(crate::application::context::TestingEnvironmentContext::id),
            Some(f.environment)
        );
        assert_ne!(f.owner, f.org);
        f.state
            .metadata
            .get_entry_by_path(&context, &EntryPath::new("public")?)
            .await?;
    }
    let control_owner: String = sqlx::query_scalar(
        "SELECT org_id FROM briefcase.testing_environments WHERE environment_id=$1",
    )
    .bind(f.environment)
    .fetch_one(&f.control)
    .await?;
    assert_eq!(control_owner, f.owner);
    let data_orgs: Vec<String> = sqlx::query_scalar(
        "SELECT org_id FROM briefcase.organizations WHERE testing_environment_id=$1",
    )
    .bind(f.environment)
    .fetch_all(&f.data)
    .await?;
    assert_eq!(data_orgs, vec![format!("{}:{}", f.environment, f.org)]);
    let production_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM briefcase.entries WHERE org_id=$1")
            .bind(&f.org)
            .fetch_one(&f.control)
            .await?;
    assert_eq!(production_rows, 0);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_bearer_rejects_wrong_org_world_audience_and_revoked_authority()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    for field in [
        "org_id",
        "testing_environment_id",
        "audience",
        "org_role",
        "tags",
        "active",
    ] {
        f.iam.reset().await;
        f.discovery(1, None).await;
        let mut body = f.introspection();
        match field {
            "org_id" => body["authorization"][field] = json!(f.owner),
            "testing_environment_id" => body["authorization"][field] = json!(Uuid::new_v4()),
            "audience" => body["authorization"][field] = json!("another-app"),
            "active" => body = json!({"active":false}),
            _ => body["authorization"][field] = Value::Null,
        }
        f.introspect(body).await;
        assert!(f.authenticate(false).await.is_err(), "accepted {field}");
        assert!(
            f.authenticate(true).await.is_err(),
            "accepted bearer {field}"
        );
    }
    f.iam.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/application/testing-context"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&f.iam)
        .await;
    assert!(f.authenticate(false).await.is_err());
    // A root selector alone never grants lifecycle control or actor authority.
    let mut headers = f.headers(false)?;
    headers.remove("authorization");
    assert!(
        extract::production_authenticate(&f.state, &headers)
            .await
            .is_err()
    );
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_delegated_folder_preserves_exact_request_and_tenant_binding()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let body = serde_json::to_vec(
        &json!({"operation_id":Uuid::new_v4(),"parent_path":"","name":"Recordings"}),
    )?;
    f.proof(&body, f.proof_response()).await;
    let (status, folder) = test_scope(
        "delegated-test".into(),
        handlers::delegated::create_folder(
            State(f.state.clone()),
            f.headers(true)?,
            Bytes::from(body.clone()),
        ),
    )
    .await?;
    assert_eq!(status, http::StatusCode::CREATED);
    let folder = serde_json::to_value(folder.0)?;
    assert_eq!(folder["org_id"], f.org);
    assert_eq!(folder["name"], "Recordings");
    // Changing bytes cannot reuse the verified request: fake IAM only accepts
    // the exact digest, method, path and selected application credential above.
    let changed = Bytes::from(serde_json::to_vec(
        &json!({"operation_id":Uuid::new_v4(),"parent_path":"","name":"Elsewhere"}),
    )?);
    assert!(
        test_scope(
            "changed-request".into(),
            handlers::delegated::create_folder(State(f.state.clone()), f.headers(true)?, changed)
        )
        .await
        .is_err()
    );
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_delegation_rejects_unbound_scope_identity_org_and_world()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let body = serde_json::to_vec(
        &json!({"operation_id":Uuid::new_v4(),"parent_path":"","name":"Rejected"}),
    )?;
    for field in [
        "missing_scope",
        "foreign_scope",
        "wrong_endpoint",
        "wrong_org",
        "wrong_world",
        "wrong_audience",
        "wrong_actor",
        "no_role",
        "no_tags",
        "revoked",
    ] {
        f.iam.reset().await;
        f.discovery(1, None).await;
        let mut proof = f.proof_response();
        match field {
            "missing_scope" => proof["authorization"]["scopes"] = f.snapshot()["scopes"].clone(),
            "foreign_scope" => {
                proof["authorization"]["scopes"][0] =
                    json!("obo:another-app:briefcase.folders.create");
            }
            "wrong_endpoint" => proof["endpoint"]["endpoint_id"] = json!("briefcase.files.create"),
            "wrong_org" => proof["org_id"] = json!(f.owner),
            "wrong_world" => {
                proof["authorization"]["testing_environment_id"] = json!(Uuid::new_v4());
            }
            "wrong_audience" => proof["audience"] = json!("another-app"),
            "wrong_actor" => proof["authorization"]["public_id"] = json!("another-carbon"),
            "no_role" => proof["authorization"]["org_role"] = Value::Null,
            "no_tags" => proof["authorization"]["tags"] = Value::Null,
            "revoked" => proof["valid"] = json!(false),
            _ => unreachable!(),
        }
        f.proof(&body, proof).await;
        assert!(
            test_scope(
                "denied-delegation".into(),
                handlers::delegated::create_folder(
                    State(f.state.clone()),
                    f.headers(true)?,
                    Bytes::from(body.clone())
                )
            )
            .await
            .is_err(),
            "accepted {field}"
        );
    }
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.entries WHERE testing_environment_id=$1",
    )
    .bind(f.environment)
    .fetch_one(&f.data)
    .await?;
    assert_eq!(rows, 0);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_reset_invalidates_data_org_context_and_erases_its_rows()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    f.introspect(f.introspection()).await;
    let context = f.authenticate(false).await?;
    f.state
        .metadata
        .get_entry_by_path(&context, &EntryPath::new("public")?)
        .await?;
    f.iam.reset().await;
    f.discovery(2, Some("2026-09-14T00:00:00Z")).await;
    extract::optional_testing_access(&f.state, &f.headers(false)?).await?;
    assert!(
        f.state
            .metadata
            .get_entry_by_path(&context, &EntryPath::new("public")?)
            .await
            .is_err()
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.entries WHERE testing_environment_id=$1",
    )
    .bind(f.environment)
    .fetch_one(&f.data)
    .await?;
    assert_eq!(rows, 0);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_public_links_route_data_org_without_granting_private_access()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    f.introspect(f.introspection()).await;
    let context = f.authenticate(false).await?;
    let public_root = f
        .state
        .metadata
        .get_entry_by_path(&context, &EntryPath::new("public")?)
        .await?;
    f.state
        .content_adapter
        .metadata_repository()
        .set_link_access(
            &context,
            public_root.id(),
            true,
            &crate::application::service::MutationMetadata::new(None, [1; 32]),
        )
        .await?;
    for headers in [HeaderMap::new(), f.headers(false)?] {
        let result = handlers::sharing::read_public(
            &f.state,
            &headers,
            &f.org,
            "public",
            handlers::sharing::PublicQuery {
                test_environment: Some(f.environment),
                ..Default::default()
            },
        )
        .await?;
        assert_eq!(result.status(), http::StatusCode::OK);
        drop(result); // releases the response's lifecycle fence
    }
    // Root-secret routing without a public UUID also retains the same public policy.
    let result = handlers::sharing::read_public(
        &f.state,
        &f.headers(false)?,
        &f.org,
        "public",
        handlers::sharing::PublicQuery::default(),
    )
    .await?;
    assert_eq!(result.status(), http::StatusCode::OK);
    drop(result);
    for (org, path, world) in [
        (f.org.as_str(), "private/test-carbon", Some(f.environment)),
        (f.owner.as_str(), "public", Some(f.environment)),
        (f.org.as_str(), "public", Some(Uuid::new_v4())),
        (f.org.as_str(), "public", None),
    ] {
        assert!(matches!(
            handlers::sharing::read_public(
                &f.state,
                &HeaderMap::new(),
                org,
                path,
                handlers::sharing::PublicQuery {
                    test_environment: world,
                    ..Default::default()
                }
            )
            .await,
            Err(AppError::NotFound)
        ));
    }
    // A contradictory secret/UUID pair cannot route to a different world.
    assert!(matches!(
        handlers::sharing::read_public(
            &f.state,
            &f.headers(false)?,
            &f.org,
            "public",
            handlers::sharing::PublicQuery {
                test_environment: Some(Uuid::new_v4()),
                ..Default::default()
            }
        )
        .await,
        Err(AppError::NotFound)
    ));
    // Current lifecycle version/pending state is checked before public entry lookup.
    sqlx::query("UPDATE briefcase.testing_environments SET iam_sync_pending=true,version=version+1 WHERE environment_id=$1")
        .bind(f.environment).execute(&f.control).await?;
    assert!(matches!(
        handlers::sharing::read_public(
            &f.state,
            &HeaderMap::new(),
            &f.org,
            "public",
            handlers::sharing::PublicQuery {
                test_environment: Some(f.environment),
                ..Default::default()
            }
        )
        .await,
        Err(AppError::NotFound)
    ));
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_raw_recording_upload_keeps_verified_data_org_and_bytes()
-> anyhow::Result<()> {
    raw_recording_upload(true).await
}

#[tokio::test]
async fn imported_world_raw_recording_upload_accepts_undisclosed_tags() -> anyhow::Result<()> {
    raw_recording_upload(false).await
}

async fn raw_recording_upload(tags_disclosed: bool) -> anyhow::Result<()> {
    use crate::application::{content::ContentService, ports::ObjectStore};
    use crate::infrastructure::s3::S3ObjectStore;
    let Some(mut f) = Fixture::new().await? else {
        return Ok(());
    };
    let storage = MockServer::start().await;
    let bytes = b"isolated recording bytes";
    let checksum = STANDARD.encode(Sha256::digest(bytes));
    Mock::given(method("PUT"))
        .and(wiremock::matchers::body_bytes(bytes.as_slice()))
        .and(header("x-amz-checksum-sha256", checksum.as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"recording-fixture\"")
                .insert_header("x-amz-version-id", "fixture-version")
                .insert_header("x-amz-checksum-sha256", checksum.as_str())
                .insert_header("x-amz-checksum-type", "FULL_OBJECT"),
        )
        .expect(1)
        .mount(&storage)
        .await;
    let objects: Arc<dyn ObjectStore> = Arc::new(S3ObjectStore::new(
        aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .credentials_provider(aws_sdk_s3::config::SharedCredentialsProvider::new(
                aws_sdk_s3::config::Credentials::new("test", "test", None, None, "fixture"),
            ))
            .build(),
        Some(storage.uri().parse()?),
        true,
    ));
    f.state.content = Arc::new(ContentService::new(
        f.state.content_adapter.clone(),
        objects,
        f.state.temporary_directory.clone(),
    ));
    let mut proof = f.proof_response();
    proof["endpoint"] =
        json!({"endpoint_id":"briefcase.files.create","path":handlers::obo::CREATE_FILE_PATH});
    proof["authorization"]["scopes"][0] = json!("obo:briefcase:briefcase.files.create");
    if !tags_disclosed {
        proof["authorization"]["scopes"] = json!([
            "obo:briefcase:briefcase.files.create",
            "self.identity.read",
            "self.membership.read"
        ]);
        proof["authorization"]["tags"] = Value::Null;
    }
    proof["metadata"] =
        json!({"path":"","name":"recording.bin","content_type":"application/octet-stream"});
    Mock::given(method("POST")).and(path("/api/v1/obo-access/verify"))
        .and(header("authorization", f.basic())).and(header("x-testing-application", f.basic()))
        .and(body_json(json!({"access_proof":PROOF,"request":{"method":"POST",
            "path":handlers::obo::CREATE_FILE_PATH,"body_sha256":hex::encode(Sha256::digest(bytes))}})))
        .respond_with(response(proof)).expect(1).mount(&f.iam).await;
    let (status, entry) = test_scope(
        "raw-recording".into(),
        handlers::obo::create_file(
            State(f.state.clone()),
            f.headers(true)?,
            axum::body::Body::from(bytes.as_slice()),
        ),
    )
    .await?;
    assert_eq!(status, http::StatusCode::CREATED);
    let entry = serde_json::to_value(entry.0)?;
    assert_eq!(entry["org_id"], f.org);
    assert_eq!(entry["origin_app_id"], ISSUER);
    assert_eq!(entry["name"], "recording.bin");
    let requests = storage
        .received_requests()
        .await
        .ok_or_else(|| anyhow::anyhow!("storage requests"))?;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, bytes);
    storage.verify().await;
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn imported_world_undisclosed_tags_preserve_directory_but_never_authorize_tag_access()
-> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let tag_id = Uuid::new_v4();
    let mut disclosed = f.introspection();
    disclosed["authorization"]["org_role"] = json!("member");
    disclosed["authorization"]["tags"] = json!([{"id":tag_id,"name":"finance"}]);
    f.introspect(disclosed.clone()).await;
    let current = f.authenticate(false).await?;
    let tag_path = EntryPath::new("finance")?;
    f.state
        .metadata
        .get_entry_by_path(&current, &tag_path)
        .await?;

    f.iam.reset().await;
    f.discovery(1, None).await;
    let mut unknown = disclosed.clone();
    unknown["scope"] = json!("self.identity.read self.membership.read");
    unknown["authorization"]["scopes"] = json!(["self.identity.read", "self.membership.read"]);
    unknown["authorization"]["tags"] = Value::Null;
    f.introspect(unknown).await;
    let current = f.authenticate(false).await?;
    assert!(current.authorization().tags().is_none());
    assert!(
        current
            .authorization()
            .iam_binding()
            .is_some_and(|binding| binding.tags.is_none())
    );
    // The exact actor owns this private tree independently of tag disclosure.
    f.state
        .metadata
        .get_entry_by_path(&current, &EntryPath::new("private/test-carbon")?)
        .await?;
    assert!(
        f.state
            .metadata
            .get_entry_by_path(&current, &tag_path)
            .await
            .is_err()
    );
    let assignments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.organization_member_tags WHERE org_id=$1 AND actor_id='test-carbon' AND tag_id=$2",
    ).bind(format!("{}:{}", f.environment, f.org)).bind(tag_id.to_string()).fetch_one(&f.data).await?;
    assert_eq!(
        assignments, 1,
        "unknown disclosure must not erase cached assignments"
    );

    f.iam.reset().await;
    f.discovery(1, None).await;
    disclosed["authorization"]["tags"] = json!([]);
    f.introspect(disclosed).await;
    let current = f.authenticate(false).await?;
    assert!(
        current
            .authorization()
            .tags()
            .is_some_and(std::collections::BTreeSet::is_empty)
    );
    f.state
        .metadata
        .get_entry_by_path(&current, &EntryPath::new("private/test-carbon")?)
        .await?;
    assert!(
        f.state
            .metadata
            .get_entry_by_path(&current, &tag_path)
            .await
            .is_err()
    );
    let assignments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM briefcase.organization_member_tags WHERE org_id=$1 AND actor_id='test-carbon'",
    ).bind(format!("{}:{}", f.environment, f.org)).fetch_one(&f.data).await?;
    assert_eq!(
        assignments, 0,
        "an explicit empty snapshot replaces assignments"
    );
    f.cleanup().await?;
    Ok(())
}
