//! What the client actually puts on the wire, checked against a mock server.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use briefcase_client::{
    AccessRight, ActorRef, ApplicationId, Client, Config, Destination, EntryUpdate, EnvironmentKey,
    IamApplicationSecret, IamEnvironmentKey, IdempotencyKey, ListEntries, NewAccessRequest,
    NewFolder, NewGrant, TestingEnvironmentCreate, TestingEnvironmentIamPairing,
    TestingEnvironmentUpdate, UpdateStatus, Upload,
};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string_contains, header, method, path, query_param},
};

fn version_document(list_entries_revision: &str) -> serde_json::Value {
    let operations: Vec<serde_json::Value> = briefcase_client::OPERATIONS
        .iter()
        .map(|operation| {
            let version = if operation.id == "listEntries" {
                list_entries_revision
            } else {
                operation.version
            };
            json!({
                "id": operation.id,
                "version": version,
                "method": operation.method,
                "path": operation.path,
            })
        })
        .collect();
    json!({
        "service": "silicon-briefcase",
        "selected_api_version": "v1",
        "supported_api_versions": ["v1"],
        "contract_version": "0.2.0",
        "build": "0.1.0",
        "operations": operations,
    })
}

fn entry_document() -> serde_json::Value {
    json!({
        "id": "01a067ce-7f19-7790-820a-0be6b3d4f828",
        "org_id": "tos",
        "type": "file",
        "visibility": "full",
        "name": "notes.md",
        "path": "private/cos:tos/notes/notes.md",
        "parent_id": "01a067ce-7f19-7790-820a-0be6b3d4f829",
        "root_type": "private",
        "tag": null,
        "content_type": "text/markdown",
        "size": 12,
        "render": "document",
        "permanent_url": "https://briefcase.example/org/tos/private/cos:tos/notes/notes.md",
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": "cos:tos"},
        "origin_app_id": null,
        "effective_access": ["read", "write", "update", "delete", "manage_permissions"],
        "created_at": "2026-09-03T10:00:00Z",
        "updated_at": "2026-09-03T10:00:00Z",
        "deleted_at": null
    })
}

fn environment_document(key: Option<&str>) -> serde_json::Value {
    let mut value = json!({
        "id": "01a067ce-7f19-7790-820a-0be6b3d4f800",
        "org_id": "tos",
        "name": "sdk-test",
        "description": "wire coverage",
        "status": "active",
        "iam_environment_id": "01a067ce-7f19-7790-820a-0be6b3d4f802",
        "iam_app_id": "tos>briefcase",
        "created_by": {"type": "carbon", "id": "cos:tester"},
        "key_generation": 1,
        "key_rotated_at": null,
        "last_activity_at": "2026-09-03T10:00:00Z",
        "cleaned_at": null,
        "deleted_at": null,
        "purge_after": null,
        "version": 1,
        "created_at": "2026-09-03T10:00:00Z",
        "updated_at": "2026-09-03T10:00:00Z"
    });
    if let Some(key) = key {
        value["key"] = json!(key);
    }
    value
}

fn tokens_document() -> serde_json::Value {
    json!({
        "access_token": "access-from-briefcase",
        "refresh_token": "refresh-from-briefcase",
        "token_type": "Bearer",
        "expires_in": 900,
        "scope": "briefcase",
        "actor": {
            "principal_id": "01a067ce-7f19-7790-820a-0be6b3d4f803",
            "type": "carbon",
            "public_id": "cos:tester"
        },
        "org_id": "tos"
    })
}

fn access_request_document() -> serde_json::Value {
    json!({
        "id": "01a067ce-7f19-7790-820a-0be6b3d4f850",
        "entry_id": "01a067ce-7f19-7790-820a-0be6b3d4f828",
        "requested_by": {"type": "carbon", "id": "cos:tester"},
        "access": ["read", "update"],
        "status": "pending",
        "created_at": "2026-09-04T00:00:00Z"
    })
}

async fn connected(server: &MockServer) -> Client {
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document("1.1.0")),
        )
        .mount(server)
        .await;

    Client::connect(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .expect("the mock base URL is valid")
            .with_token("test-token")
            .with_auto_update(false),
    )
    .await
    .expect("a matching deployment must connect")
}

#[tokio::test]
async fn connecting_refuses_a_deployment_serving_another_revision() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document("9.0.0")),
        )
        .mount(&server)
        .await;

    let error = Client::connect(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .await
    .expect_err("a changed revision must refuse the connection");

    assert!(error.to_string().contains("listEntries"));
}

#[tokio::test]
async fn contract_negotiation_defers_maintenance_until_an_ordinary_request() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    assert_eq!(client.update_status(), UpdateStatus::NotChecked);

    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "next_cursor": null
        })))
        .mount(&server)
        .await;
    client
        .list_entries(&ListEntries::default())
        .await
        .expect("an ordinary request must still be sent");

    assert_eq!(client.update_status(), UpdateStatus::Disabled);
}

#[tokio::test]
async fn negotiation_header_and_body_must_agree() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v2")
                .set_body_json(version_document("1.1.0")),
        )
        .mount(&server)
        .await;

    let error = Client::connect(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .await
    .expect_err("a proxy must not be able to disagree about the selected API major");
    assert!(error.to_string().contains("header"));
    assert!(error.to_string().contains("body"));

    let missing = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(version_document("1.1.0")))
        .mount(&missing)
        .await;
    let error = Client::connect(
        Config::new(&format!("{}/api/v1/", missing.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .await
    .expect_err("the selected API header is mandatory");
    assert!(error.to_string().contains("omitted"));
}

#[tokio::test]
async fn contract_handshake_never_follows_a_redirect_to_another_origin() {
    let server = MockServer::start().await;
    let other_origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/api/version", other_origin.uri())),
        )
        .mount(&server)
        .await;

    Client::connect(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .await
    .expect_err("a deployment redirect must not select another trust boundary");
    assert!(
        other_origin
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn every_request_carries_the_tenant_and_the_bearer() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(header("x-org-id", "tos"))
        .and(header("authorization", "Bearer test-token"))
        .and(query_param("limit", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [entry_document()], "next_cursor": "next"})),
        )
        .mount(&server)
        .await;

    let page = client
        .list_entries(&ListEntries::default().limit(2))
        .await
        .expect("the listing must be served");

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next_cursor.as_deref(), Some("next"));
    assert_eq!(page.items[0].name, "notes.md");
}

#[tokio::test]
async fn creating_a_folder_sends_its_container_and_an_idempotency_key() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/entries"))
        .and(header("content-type", "application/json"))
        .and(body_string_contains("\"root_type\":\"tag\""))
        .and(body_string_contains("\"tag\":\"engineering\""))
        .and(body_string_contains("\"principal\""))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    let folder = NewFolder::in_tag("specs", "engineering").inviting([NewGrant::new(
        ActorRef::carbon("cos:tos"),
        [AccessRight::Read, AccessRight::Write],
    )
    .inheriting()]);
    client
        .create_folder(&folder)
        .await
        .expect("the folder must be created");

    let requests = server.received_requests().await.unwrap_or_default();
    let create = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/entries")
        .expect("the creation must have been sent");
    assert!(create.headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn access_requests_use_distinct_uuid_and_hidden_path_routes() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let entry_id = "01a067ce-7f19-7790-820a-0be6b3d4f828";
    let hidden_path = "private/cos:owner/hidden/report.pdf";
    let request =
        NewAccessRequest::new([AccessRight::Read, AccessRight::Update]).because("quarterly review");

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/entries/{entry_id}/access-requests")))
        .and(header("x-org-id", "tos"))
        .and(header("authorization", "Bearer test-token"))
        .and(body_json(json!({
            "access": ["read", "update"],
            "reason": "quarterly review"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(access_request_document()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/access-requests"))
        .and(header("x-org-id", "tos"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("idempotency-key", "path-access-attempt-0001"))
        .and(body_json(json!({
            "path": hidden_path,
            "access": ["read", "update"],
            "reason": "quarterly review"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(access_request_document()))
        .expect(1)
        .mount(&server)
        .await;

    client
        .request_access(entry_id.parse().unwrap(), &request)
        .await
        .unwrap();
    client
        .request_access_by_path_with_key(
            hidden_path,
            &request,
            &IdempotencyKey::new("path-access-attempt-0001").unwrap(),
        )
        .await
        .unwrap();

    let application_requests: Vec<_> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert_eq!(application_requests.len(), 2);
    assert!(application_requests.iter().all(|request| {
        request.url.path() != format!("/api/v1/org/tos/{hidden_path}")
            && request.url.path() != format!("/api/v1/entries/{entry_id}")
    }));
}

#[tokio::test]
async fn an_upload_is_multipart_with_the_destination_and_the_file() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains("name=\"path\""))
        .and(body_string_contains("private/cos:tos/notes"))
        .and(body_string_contains("filename=\"notes.md\""))
        .and(body_string_contains("first revision"))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    let upload = Upload::bytes(
        Destination::path("private/cos:tos/notes"),
        "notes.md",
        b"first revision".to_vec(),
    )
    .with_content_type("text/markdown");
    let entry = client
        .upload(&upload)
        .await
        .expect("the upload must be accepted");

    assert_eq!(entry.name, "notes.md");
}

#[tokio::test]
async fn entry_update_and_version_restore_accept_durable_retry_keys() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let entry_id = "01a067ce-7f19-7790-820a-0be6b3d4f828";
    let version_id = "01a067ce-7f19-7790-820a-0be6b3d4f899";

    Mock::given(method("PATCH"))
        .and(path(format!("/api/v1/entries/{entry_id}")))
        .and(header("idempotency-key", "move-attempt-0001"))
        .and(body_json(json!({"name": "renamed.md"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/entries/{entry_id}/versions/{version_id}/restore"
        )))
        .and(header("idempotency-key", "restore-version-attempt-0001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .mount(&server)
        .await;

    client
        .update_entry_with_key(
            entry_id.parse().unwrap(),
            &EntryUpdate::rename("renamed.md"),
            &IdempotencyKey::new("move-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client
        .restore_version_with_key(
            entry_id.parse().unwrap(),
            version_id.parse().unwrap(),
            &IdempotencyKey::new("restore-version-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_refusal_keeps_its_code_request_id_and_retry_delay() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3600")
                .set_body_json(json!({
                    "error": {
                        "code": "daily_upload_limit_exhausted",
                        "message": "The organization has uploaded its daily allowance.",
                        "request_id": "01a067ce-0000-7000-8000-000000000001"
                    }
                })),
        )
        .mount(&server)
        .await;

    let error = client
        .upload(&Upload::bytes(
            Destination::path("public"),
            "big.bin",
            vec![0_u8; 8],
        ))
        .await
        .expect_err("a spent allowance must be refused");

    assert_eq!(error.code(), Some("daily_upload_limit_exhausted"));
    assert!(error.is_retryable());
    assert_eq!(
        error.retry_after(),
        Some(std::time::Duration::from_secs(3600))
    );
    assert!(
        error
            .to_string()
            .contains("01a067ce-0000-7000-8000-000000000001")
    );
}

#[tokio::test]
async fn a_hidden_entry_reads_exactly_like_a_missing_one() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path("/org/tos/private/cos:tos/notes/notes.md"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "code": "not_found",
                "message": "The requested resource was not found.",
                "request_id": "01a067ce-0000-7000-8000-000000000002"
            }
        })))
        .mount(&server)
        .await;

    let error = client
        .entry_at("private/cos:tos/notes/notes.md")
        .await
        .expect_err("an entry the caller cannot read must be missing");

    assert!(error.is_not_found());
    assert!(!error.is_forbidden());
}

#[tokio::test]
async fn an_application_sends_its_proof_and_never_a_bearer() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    assert_eq!(client.update_status(), UpdateStatus::NotChecked);

    Mock::given(method("POST"))
        .and(path("/api/v1/obo/files"))
        .and(header("x-app-id", "acme>app-notes"))
        .and(header("x-iam-obo-access-proof", "proof-abc"))
        .and(header("content-type", "application/octet-stream"))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    client
        .create_file_on_behalf_of(&briefcase_client::OnBehalfOfUpload::bytes(
            "acme>app-notes",
            "proof-abc",
            b"written by an application".to_vec(),
        ))
        .await
        .expect("the application file must be created");
    assert_eq!(client.update_status(), UpdateStatus::NotChecked);

    let requests = server.received_requests().await.unwrap_or_default();
    let obo = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/obo/files")
        .expect("the on-behalf-of call must have been sent");
    // Presenting both credentials at once is a request error, so the client
    // must not send its bearer token here even when it holds one.
    assert!(!obo.headers.contains_key("authorization"));
}

#[tokio::test]
async fn a_range_read_asks_for_exactly_those_bytes() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/entries/01a067ce-7f19-7790-820a-0be6b3d4f828/content",
        ))
        .and(header("range", "bytes=2-8"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 2-8/44")
                .set_body_bytes(b"otes.md".to_vec()),
        )
        .mount(&server)
        .await;

    let entry_id = "01a067ce-7f19-7790-820a-0be6b3d4f828".parse().unwrap();
    let stream = client
        .read_content(entry_id, Some(briefcase_client::ByteRange::inclusive(2, 8)))
        .await
        .expect("the range must be served");

    assert_eq!(stream.content_range(), Some("bytes 2-8/44"));
    assert_eq!(stream.bytes().await.unwrap(), b"otes.md");
}

#[tokio::test]
async fn a_testing_key_selects_the_plane_without_replacing_identity() {
    let server = MockServer::start().await;
    let root_key = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6";

    Mock::given(method("GET"))
        .and(path("/api/version"))
        .and(header("x-testing-environment-key", root_key))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document("1.1.0")),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(header("x-testing-environment-key", root_key))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "next_cursor": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/files"))
        .and(header("x-testing-environment-key", root_key))
        .and(header("x-app-id", "tos>notes"))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    let client = Client::connect(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_token("test-token")
            .with_environment(EnvironmentKey::new(root_key).unwrap())
            .with_auto_update(false),
    )
    .await
    .unwrap();
    client.list_entries(&ListEntries::default()).await.unwrap();
    client
        .create_file_on_behalf_of(&briefcase_client::OnBehalfOfUpload::bytes(
            "tos>notes",
            "proof",
            b"body".to_vec(),
        ))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap_or_default();
    let obo = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/obo/files")
        .unwrap();
    assert!(!obo.headers.contains_key("authorization"));
}

#[tokio::test]
async fn test_only_self_operations_fail_locally_without_a_key() {
    let server = MockServer::start().await;
    let client = Client::new_unchecked(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .unwrap();

    let error = client.current_testing_environment().await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid client configuration: this action is only possible for a test environment"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn production_environment_management_fails_locally_with_a_test_key() {
    let server = MockServer::start().await;
    let environment_id = "01a067ce-7f19-7790-820a-0be6b3d4f800".parse().unwrap();
    let client = Client::new_unchecked(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_token("test-token")
            .with_environment(EnvironmentKey::new("B2345678901234567890123456789012").unwrap())
            .with_auto_update(false),
    )
    .unwrap();
    let input = TestingEnvironmentCreate::new(
        "sdk-test",
        environment_id,
        IamEnvironmentKey::new("a2345678901234567890123456789012").unwrap(),
        ApplicationId::new("tos>briefcase").unwrap(),
        IamApplicationSecret::new(format!("ask_{}", "a".repeat(43))).unwrap(),
    );
    let update = TestingEnvironmentUpdate {
        name: Some("changed".to_owned()),
        description: None,
    };
    let pairing = TestingEnvironmentIamPairing::new(
        environment_id,
        IamEnvironmentKey::new("c2345678901234567890123456789012").unwrap(),
        ApplicationId::new("tos>briefcase").unwrap(),
        IamApplicationSecret::new(format!("ask_{}", "b".repeat(43))).unwrap(),
    );
    let key = IdempotencyKey::new("management-attempt-0001").unwrap();

    let errors = [
        client.testing_environments(None).await.unwrap_err(),
        client
            .create_testing_environment_with_key(&input, &key)
            .await
            .unwrap_err(),
        client
            .testing_environment(environment_id)
            .await
            .unwrap_err(),
        client
            .update_testing_environment_with_key(environment_id, 1, &update, &key)
            .await
            .unwrap_err(),
        client
            .delete_testing_environment_with_key(environment_id, &key)
            .await
            .unwrap_err(),
        client
            .restore_testing_environment_with_key(environment_id, &key)
            .await
            .unwrap_err(),
        client
            .testing_environment_key(environment_id)
            .await
            .unwrap_err(),
        client
            .rotate_testing_environment_key_with_key(environment_id, &key)
            .await
            .unwrap_err(),
        client
            .replace_testing_environment_iam_pairing_with_key(environment_id, &pairing, &key)
            .await
            .unwrap_err(),
        client
            .clean_testing_environment_with_key(environment_id, &key)
            .await
            .unwrap_err(),
    ];
    for error in errors {
        assert_eq!(
            error.to_string(),
            "invalid client configuration: testing-environment management is only possible from the production plane"
        );
    }
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn creating_an_environment_carries_all_iam_bootstrap_credentials_once() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let body = json!({
        "name": "sdk-test",
        "description": "wire coverage",
        "iam_environment_id": "01a067ce-7f19-7790-820a-0be6b3d4f802",
        "iam_environment_key": "a2345678901234567890123456789012",
        "iam_app_id": "tos>briefcase",
        "iam_app_secret": format!("ask_{}", "a".repeat(43)),
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/tos/testing-environments"))
        .and(header("authorization", "Bearer test-token"))
        .and(header("idempotency-key", "create-env-attempt-0001"))
        .and(body_json(body))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(environment_document(Some(
                "B2345678901234567890123456789012",
            ))),
        )
        .mount(&server)
        .await;

    let input = TestingEnvironmentCreate::new(
        "sdk-test",
        "01a067ce-7f19-7790-820a-0be6b3d4f802".parse().unwrap(),
        IamEnvironmentKey::new("a2345678901234567890123456789012").unwrap(),
        ApplicationId::new("tos>briefcase").unwrap(),
        IamApplicationSecret::new(format!("ask_{}", "a".repeat(43))).unwrap(),
    )
    .described("wire coverage");
    let idempotency_key = IdempotencyKey::new("create-env-attempt-0001").unwrap();
    let created = client
        .create_testing_environment_with_key(&input, &idempotency_key)
        .await
        .unwrap();

    assert_eq!(created.environment.name, "sdk-test");
    assert_eq!(
        created.key.expose_secret(),
        "B2345678901234567890123456789012"
    );
}

#[tokio::test]
async fn slt_exchange_and_refresh_are_anonymous_and_stay_in_the_selected_plane() {
    let server = MockServer::start().await;
    let root_key = "B2345678901234567890123456789012";
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/slt"))
        .and(header("x-testing-environment-key", root_key))
        .and(header("idempotency-key", "login-attempt-0001"))
        .and(body_json(json!({"slt": "slt-once"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens_document()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .and(header("x-testing-environment-key", root_key))
        .and(header("idempotency-key", "refresh-attempt-0001"))
        .and(body_json(
            json!({"refresh_token": "refresh-from-briefcase"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(tokens_document()))
        .mount(&server)
        .await;

    let client = Client::new_unchecked(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_token("must-not-leak")
            .with_environment(EnvironmentKey::new(root_key).unwrap())
            .with_auto_update(false),
    )
    .unwrap();
    let too_short = IdempotencyKey::new("short-00").unwrap();
    assert!(
        client
            .login_with_slt_with_key("slt-once", &too_short)
            .await
            .is_err()
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    let too_long = IdempotencyKey::new("x".repeat(256)).unwrap_err();
    assert!(too_long.to_string().contains("255"));

    let login_key = IdempotencyKey::new("login-attempt-0001").unwrap();
    let login = client
        .login_with_slt_with_key("slt-once", &login_key)
        .await
        .unwrap();
    let refresh_key = IdempotencyKey::new("refresh-attempt-0001").unwrap();
    client
        .refresh_session_with_key(&login.refresh_token, &refresh_key)
        .await
        .unwrap();
    assert_eq!(client.update_status(), UpdateStatus::NotChecked);

    let requests = server.received_requests().await.unwrap_or_default();
    for request in requests {
        assert!(!request.headers.contains_key("authorization"));
        assert!(request.headers.contains_key("idempotency-key"));
    }
}

#[tokio::test]
async fn slt_exchange_rejects_unbound_or_cross_organization_sessions() {
    for (returned_organization, expected_message) in [
        (None, "organization-unbound"),
        (Some("other"), "organization other"),
    ] {
        let server = MockServer::start().await;
        let mut response = tokens_document();
        if let Some(organization) = returned_organization {
            response["org_id"] = json!(organization);
        } else {
            response
                .as_object_mut()
                .expect("the token response is an object")
                .remove("org_id");
        }
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/slt"))
            .and(header("idempotency-key", "login-org-check-0001"))
            .and(body_json(json!({"slt": "slt-once"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::new_unchecked(
            Config::new(&format!("{}/api/v1/", server.uri()), "tos")
                .unwrap()
                .with_auto_update(false),
        )
        .unwrap();
        let key = IdempotencyKey::new("login-org-check-0001").unwrap();
        let error = client
            .login_with_slt_with_key("slt-once", &key)
            .await
            .unwrap_err();

        assert!(matches!(&error, briefcase_client::Error::Protocol(_)));
        assert!(error.to_string().contains(expected_message));
        assert!(error.to_string().contains("configured for tos"));
        let requests = server.received_requests().await.unwrap_or_default();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].headers.contains_key("authorization"));
    }
}

#[tokio::test]
async fn noncanonical_obo_application_ids_fail_before_a_request() {
    let server = MockServer::start().await;
    let client = Client::new_unchecked(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_auto_update(false),
    )
    .unwrap();
    let error = client
        .create_file_on_behalf_of(&briefcase_client::OnBehalfOfUpload::bytes(
            "notes",
            "proof",
            b"body".to_vec(),
        ))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("canonical"));
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one scenario intentionally walks every lifecycle route against one mock plane"
)]
async fn the_complete_environment_lifecycle_matches_the_server_contract() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let id = "01a067ce-7f19-7790-820a-0be6b3d4f800";
    let base = format!("/api/v1/organizations/tos/testing-environments/{id}");
    let root_key = "B2345678901234567890123456789012";

    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/tos/testing-environments"))
        .and(query_param("status", "all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [environment_document(None)]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(base.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(environment_document(None)))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(base.clone()))
        .and(header("if-match", "\"1\""))
        .and(header("idempotency-key", "update-env-attempt-0001"))
        .and(header("content-type", "application/merge-patch+json"))
        .and(body_json(json!({"name": "renamed", "description": null})))
        .respond_with(ResponseTemplate::new(200).set_body_json(environment_document(None)))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(base.clone()))
        .and(header("idempotency-key", "delete-env-attempt-0001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(environment_document(None)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/key")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "environment_id": id,
            "key_generation": 1,
            "key_rotated_at": null,
            "key": root_key
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/key-rotations")))
        .and(header("idempotency-key", "rotate-env-attempt-0001"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(environment_document(Some(root_key))),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/iam-pairings")))
        .and(header("idempotency-key", "pair-iam-attempt-0001"))
        .and(body_json(json!({
            "iam_environment_id": "01a067ce-7f19-7790-820a-0be6b3d4f899",
            "iam_environment_key": "c2345678901234567890123456789012",
            "iam_app_id": "tos>briefcase",
            "iam_app_secret": format!("ask_{}", "b".repeat(43)),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(environment_document(None)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/restorations")))
        .and(header("idempotency-key", "restore-env-attempt-0001"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(environment_document(Some(root_key))),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{base}/cleanings")))
        .and(header("idempotency-key", "clean-env-attempt-0001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "environment_id": id,
            "erased_rows": 17,
            "cleaned_at": "2026-09-03T11:00:00Z"
        })))
        .mount(&server)
        .await;

    let parsed_id = id.parse().unwrap();
    assert_eq!(
        client
            .testing_environments(Some("all"))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let environment = client.testing_environment(parsed_id).await.unwrap();
    assert_eq!(environment.created_by.id, "cos:tester");
    client
        .update_testing_environment_with_key(
            parsed_id,
            1,
            &briefcase_client::TestingEnvironmentUpdate {
                name: Some("renamed".to_owned()),
                description: Some(None),
            },
            &IdempotencyKey::new("update-env-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client
        .delete_testing_environment_with_key(
            parsed_id,
            &IdempotencyKey::new("delete-env-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client.testing_environment_key(parsed_id).await.unwrap();
    client
        .rotate_testing_environment_key_with_key(
            parsed_id,
            &IdempotencyKey::new("rotate-env-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client
        .replace_testing_environment_iam_pairing_with_key(
            parsed_id,
            &TestingEnvironmentIamPairing::new(
                "01a067ce-7f19-7790-820a-0be6b3d4f899".parse().unwrap(),
                IamEnvironmentKey::new("c2345678901234567890123456789012").unwrap(),
                ApplicationId::new("tos>briefcase").unwrap(),
                IamApplicationSecret::new(format!("ask_{}", "b".repeat(43))).unwrap(),
            ),
            &IdempotencyKey::new("pair-iam-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client
        .clean_testing_environment_with_key(
            parsed_id,
            &IdempotencyKey::new("clean-env-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    let restored = client
        .restore_testing_environment_with_key(
            parsed_id,
            &IdempotencyKey::new("restore-env-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.key.expose_secret(), root_key);

    let sandbox = Client::new_unchecked(
        Config::new(&format!("{}/api/v1/", server.uri()), "tos")
            .unwrap()
            .with_environment(EnvironmentKey::new(root_key).unwrap())
            .with_auto_update(false),
    )
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/testing-environment"))
        .and(header("x-testing-environment-key", root_key))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id,
            "name": "sdk-test",
            "description": "wire coverage",
            "key_generation": 1,
            "created_at": "2026-09-03T10:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/testing-environment/cleanings"))
        .and(header("x-testing-environment-key", root_key))
        .and(header("idempotency-key", "clean-current-attempt-0001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "environment_id": id,
            "erased_rows": 0,
            "cleaned_at": "2026-09-03T11:00:00Z"
        })))
        .mount(&server)
        .await;
    sandbox.current_testing_environment().await.unwrap();
    sandbox
        .clean_current_testing_environment_with_key(
            &IdempotencyKey::new("clean-current-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
}
