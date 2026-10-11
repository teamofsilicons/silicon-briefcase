//! What the client actually puts on the wire, checked against a mock server.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use briefcase_client::{
    AccessRight, ActorRef, Client, Config, Destination, EntryUpdate, IdempotencyKey, ListEntries,
    NewFolder, NewGrant, PermissionQuery, Upload,
};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string_contains, header, method, path, query_param},
};

const ENTRY_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f828";

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
        "contract_version": "4.1.0",
        "build": "4.1.0",
        "operations": operations,
    })
}

fn entry_document() -> serde_json::Value {
    json!({
        "id": ENTRY_ID,
        "org_id": "zQo",
        "type": "file",
        "visibility": "full",
        "name": "plan.md",
        "path": "notes/plan.md",
        "parent_id": "01a067ce-7f19-7790-820a-0be6b3d4f829",
        "root_type": "private",
        "tag": null,
        "content_type": "text/markdown",
        "size": 12,
        "render": "document",
        "permanent_url": "https://briefcase.example/c:ada/notes/plan.md",
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": "zQo"},
        "origin_app_id": null,
        "effective_access": ["read", "write", "update", "delete", "manage_permissions"],
        "created_at": "2026-10-10T10:00:00Z",
        "updated_at": "2026-10-10T10:00:00Z",
        "deleted_at": null
    })
}

fn base(server: &MockServer) -> String {
    format!("{}/api/v1/", server.uri())
}

async fn connected(server: &MockServer) -> Client {
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document(
                    briefcase_client::OPERATIONS
                        .iter()
                        .find(|operation| operation.id == "listEntries")
                        .unwrap()
                        .version,
                )),
        )
        .mount(server)
        .await;

    Client::connect(
        Config::new(&base(server))
            .expect("the mock base URL is valid")
            .with_token("test-token"),
    )
    .await
    .expect("a matching deployment must connect")
}

#[test]
fn the_client_carries_the_four_point_one_operation_catalogue() {
    assert_eq!(briefcase_client::OPERATIONS.len(), 56);
    assert_eq!(briefcase_client::CONTRACT_VERSION, "4.1.0");
    for retired in [
        "submitTelemetry",
        "readIamInfo",
        "readLoginStatus",
        "exchangeShortLivedToken",
        "refreshApplicationSession",
        "listTestingEnvironments",
    ] {
        assert!(
            briefcase_client::OPERATIONS
                .iter()
                .all(|operation| operation.id != retired),
            "{retired} is retired"
        );
    }
    for added in [
        "readAccountsInfo",
        "readAccount",
        "listDrives",
        "listSharedWithMe",
        "resolveDriveTop",
        "listArchiveContents",
    ] {
        assert!(
            briefcase_client::OPERATIONS
                .iter()
                .any(|operation| operation.id == added),
            "{added} is part of contract 4.1.0"
        );
    }
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

    let error = Client::connect(Config::new(&base(&server)).unwrap())
        .await
        .expect_err("a changed revision must refuse the connection");

    assert!(error.to_string().contains("listEntries"));
}

#[tokio::test]
async fn negotiation_header_and_body_must_agree() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v2")
                .set_body_json(version_document("2.0.0")),
        )
        .mount(&server)
        .await;

    let error = Client::connect(Config::new(&base(&server)).unwrap())
        .await
        .expect_err("a proxy must not be able to disagree about the selected API major");
    assert!(error.to_string().contains("header"));
    assert!(error.to_string().contains("body"));

    let missing = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(version_document("2.0.0")))
        .mount(&missing)
        .await;
    let error = Client::connect(Config::new(&base(&missing)).unwrap())
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

    Client::connect(Config::new(&base(&server)).unwrap())
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
async fn every_request_carries_the_bearer_and_no_organization_or_test_header() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
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
    assert_eq!(page.items[0].drive, "zQo");
    let requests = server.received_requests().await.unwrap();
    for request in &requests {
        for retired in ["x-org-id", "x-briefcase-app-secret", "x-app-id"] {
            assert!(!request.headers.contains_key(retired), "{retired}");
        }
    }
    let listing = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/entries")
        .unwrap();
    // The top of the caller's own drive names no drive and no path.
    assert!(listing.url.query_pairs().all(|(key, _)| key == "limit"));
}

#[tokio::test]
async fn listing_another_drive_names_its_owner_and_the_path_inside_it() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(query_param("drive", "si:scout"))
        .and(query_param("path", "reports/2026"))
        .and(query_param("filter", "is:pdf"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let query = ListEntries {
        filter: Some("is:pdf".to_owned()),
        ..ListEntries::in_folder(Destination::in_drive("si:scout", "/reports/2026/"))
    };
    client.list_entries(&query).await.unwrap();
}

#[tokio::test]
async fn creating_a_folder_names_its_drive_and_parent_path_and_sends_an_idempotency_key() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/entries"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "name": "drafts",
            "parent_path": "notes",
            "drive": "si:scout",
            "invitees": [{"principal": {"type": "carbon", "id": "c:ada"}, "access": ["read", "write"], "inherit": true}]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/entries"))
        .and(body_json(json!({"name": "top", "invitees": []})))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;

    let folder = NewFolder::in_folder("drafts", Destination::in_drive("si:scout", "notes"))
        .inviting([NewGrant::new(
            ActorRef::carbon("c:ada"),
            [AccessRight::Read, AccessRight::Write],
        )
        .inheriting()]);
    client.create_folder(&folder).await.unwrap();
    client
        .create_folder(&NewFolder::at_top("top"))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        requests
            .iter()
            .filter(|request| request.url.path() == "/api/v1/entries")
            .all(|request| request.headers.contains_key("idempotency-key"))
    );
}

#[tokio::test]
async fn an_upload_is_multipart_with_the_drive_destination_and_the_file() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains("name=\"drive\""))
        .and(body_string_contains("c:ada"))
        .and(body_string_contains("name=\"path\""))
        .and(body_string_contains("shared/plans"))
        .and(body_string_contains("filename=\"plan.md\""))
        .and(body_string_contains("first revision"))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;

    let upload = Upload::bytes(
        Destination::in_drive("c:ada", "shared/plans"),
        "plan.md",
        b"first revision".to_vec(),
    )
    .with_content_type("text/markdown");
    let entry = client.upload(&upload).await.unwrap();
    assert_eq!(entry.name, "plan.md");

    // The top of the caller's own drive sends neither a drive nor a path.
    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains("filename=\"top.md\""))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    client
        .upload(&Upload::bytes(Destination::top(), "top.md", b"x".to_vec()))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let top = requests
        .iter()
        .rev()
        .find(|request| request.url.path() == "/api/v1/uploads")
        .unwrap();
    let body = String::from_utf8_lossy(&top.body);
    assert!(!body.contains("name=\"path\""), "{body}");
    assert!(!body.contains("name=\"drive\""), "{body}");
}

#[tokio::test]
async fn entry_update_and_version_restore_accept_durable_retry_keys() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let version_id = "01a067ce-7f19-7790-820a-0be6b3d4f899";

    Mock::given(method("PATCH"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}")))
        .and(header("idempotency-key", "move-attempt-0001"))
        .and(body_json(json!({"name": "renamed.md"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/entries/{ENTRY_ID}/versions/{version_id}/restore"
        )))
        .and(header("idempotency-key", "restore-version-attempt-0001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .mount(&server)
        .await;

    client
        .update_entry_with_key(
            ENTRY_ID.parse().unwrap(),
            &EntryUpdate::rename("renamed.md"),
            &IdempotencyKey::new("move-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
    client
        .restore_version_with_key(
            ENTRY_ID.parse().unwrap(),
            version_id.parse().unwrap(),
            &IdempotencyKey::new("restore-version-attempt-0001").unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_refusal_keeps_its_code_hint_request_id_and_retry_delay() {
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
                        "message": "This drive has uploaded its daily allowance.",
                        "hint": "Try again after midnight UTC.",
                        "request_id": "01a067ce-0000-7000-8000-000000000001"
                    }
                })),
        )
        .mount(&server)
        .await;

    let error = client
        .upload(&Upload::bytes(Destination::top(), "big.bin", vec![0_u8; 8]))
        .await
        .expect_err("a spent allowance must be refused");

    assert_eq!(error.code(), Some("daily_upload_limit_exhausted"));
    assert_eq!(error.hint(), Some("Try again after midnight UTC."));
    assert_eq!(error.status(), Some(429));
    assert!(error.is_retryable());
    assert_eq!(
        error.retry_after(),
        Some(std::time::Duration::from_hours(1))
    );
    assert!(
        error
            .to_string()
            .contains("01a067ce-0000-7000-8000-000000000001")
    );
}

#[tokio::test]
async fn effective_access_names_the_drive_its_paths_are_in() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/permissions/effective"))
        .and(body_json(
            json!({"entry_ids": [], "paths": ["notes"], "drive": "c:ada"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "unresolved_entry_ids": [], "unresolved_paths": ["notes"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let inspection = client
        .effective_access(&PermissionQuery::paths(["notes".to_owned()]).in_drive("c:ada"))
        .await
        .unwrap();
    assert_eq!(inspection.unresolved_paths, vec!["notes"]);
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({
        "error": {
            "code": "not_found",
            "message": "The requested resource was not found.",
            "request_id": "01a067ce-0000-7000-8000-000000000002"
        }
    }))
}

#[tokio::test]
async fn an_address_resolves_by_owner_id_and_path_and_follows_one_move() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path("/api/v1/paths/c:ada/notes/plan.md"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .entry_at("c:ada", "/notes/plan.md")
            .await
            .unwrap()
            .name,
        "plan.md"
    );

    // An id the owner gave up moves to the owner's uuid, once.
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/c:old-ada/notes/plan.md"))
        .respond_with(ResponseTemplate::new(308).set_body_json(json!({
            "moved_to": format!("{}/api/v1/paths/c:ada/notes/plan.md", server.uri()),
            "permanent_url": "https://briefcase.example/c:ada/notes/plan.md",
            "owner": {"uuid": "zQo", "id": "c:ada"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/zQo/notes/plan.md"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    let moved = client.entry_at("c:old-ada", "notes/plan.md").await.unwrap();
    assert_eq!(moved.drive, "zQo");

    // The top of a drive is its own address.
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/si:scout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    client.entry_at("si:scout", "").await.unwrap();
}

#[tokio::test]
async fn an_address_that_keeps_moving_is_refused_rather_than_chased() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let moved = ResponseTemplate::new(308).set_body_json(json!({
        "moved_to": format!("{}/api/v1/paths/zQo/a.md", server.uri()),
        "permanent_url": "https://briefcase.example/c:ada/a.md",
        "owner": {"uuid": "zQo", "id": "c:ada"}
    }));
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/c:old/a.md"))
        .respond_with(moved.clone())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/zQo/a.md"))
        .respond_with(moved)
        .mount(&server)
        .await;
    let error = client.entry_at("c:old", "a.md").await.unwrap_err();
    assert!(
        matches!(error, briefcase_client::Error::Protocol(_)),
        "{error}"
    );
}

#[tokio::test]
async fn a_hidden_entry_reads_exactly_like_a_missing_one() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/paths/c:ada/private/notes.md"))
        .respond_with(not_found())
        .mount(&server)
        .await;
    let error = client
        .entry_at("c:ada", "private/notes.md")
        .await
        .expect_err("an entry the caller cannot read must be missing");
    assert!(error.is_not_found());
    assert!(!error.is_forbidden());
}

#[tokio::test]
async fn an_address_from_before_personal_drives_resolves_at_the_origin() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path("/org/tos/public/handbook.md"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    let entry = client
        .entry_at_legacy_address("tos", "public/handbook.md")
        .await
        .unwrap();
    assert_eq!(
        entry.permanent_url.as_str(),
        "https://briefcase.example/c:ada/notes/plan.md"
    );
}

#[tokio::test]
async fn reads_ask_for_exactly_the_bytes_and_addresses_ask_for_inline_content() {
    let server = MockServer::start().await;
    let client = connected(&server).await;

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/content")))
        .and(header("range", "bytes=2-8"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("content-range", "bytes 2-8/44")
                .set_body_bytes(b"otes.md".to_vec()),
        )
        .mount(&server)
        .await;
    let stream = client
        .read_content(
            ENTRY_ID.parse().unwrap(),
            Some(briefcase_client::ByteRange::inclusive(2, 8)),
        )
        .await
        .unwrap();
    assert_eq!(stream.content_range(), Some("bytes 2-8/44"));
    assert_eq!(stream.bytes().await.unwrap(), b"otes.md");

    Mock::given(method("GET"))
        .and(path("/api/v1/paths/c:ada/notes/plan.md"))
        .and(query_param("disposition", "inline"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"# plan".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    let content = client
        .read_content_at("c:ada", "notes/plan.md")
        .await
        .unwrap();
    assert_eq!(content.bytes().await.unwrap(), b"# plan");
}

#[tokio::test]
async fn an_archive_is_listed_without_extracting_it() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/archive")))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "format": "zip",
            "entries": [
                {"path": "docs/", "type": "folder", "size": 0},
                {"path": "docs/a.txt", "type": "file", "size": 5, "compressed_size": 3,
                 "modified_at": "2026-10-10T10:00:00Z", "encrypted": true},
            ],
            "truncated": true,
            "truncated_because": "entry_limit"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let listing = client
        .archive_contents(ENTRY_ID.parse().unwrap(), Some(2))
        .await
        .unwrap();
    assert_eq!(listing.format, "zip");
    assert_eq!(listing.entries.len(), 2);
    assert!(listing.entries[1].encrypted);
    assert_eq!(listing.truncated_because.as_deref(), Some("entry_limit"));
}

#[tokio::test]
async fn the_account_its_drives_and_what_others_shared_are_read_with_the_bearer() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    let summary = json!({"uuid": "zQo", "id": "c:ada", "kind": "carbon", "display_name": "Ada", "pfp_url": null});
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uuid": "Sc0", "id": "si:scout", "kind": "silicon", "display_name": "Scout",
            "pfp_url": null, "custodian": {"uuid": "zQo", "id": "c:ada"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/drives"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [
            {"owner": summary, "relation": "own",
             "root_id": "01a067ce-7f19-7790-820a-0be6b3d4f801",
             "permanent_url": "https://briefcase.example/c:ada"}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/shared"))
        .and(query_param("cursor", "next"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [
            {"owner": summary, "shared_at": "2026-10-10T10:00:00Z", "entry": entry_document()}
        ], "next_cursor": null})))
        .expect(1)
        .mount(&server)
        .await;

    let me = client.me().await.unwrap();
    assert_eq!(me.display_id(), "si:scout");
    assert_eq!(me.custodian.unwrap().uuid, "zQo");
    let drives = client.drives().await.unwrap();
    assert_eq!(drives.items[0].owner.display_id(), "c:ada");
    let shared = client.shared(Some("next"), Some(10)).await.unwrap();
    assert_eq!(shared.items[0].entry.name, "plan.md");
}

#[tokio::test]
async fn discovery_needs_no_credential() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id": "briefcase",
            "accounts_url": "https://accounts.teamofsilicons.com",
            "delegated_scopes": ["briefcase.files.read"]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::new_unchecked(
        Config::new(&base(&server))
            .unwrap()
            .with_token("must-not-be-sent"),
    )
    .unwrap();
    let info = client.accounts_info().await.unwrap();
    assert_eq!(info.app_id, "briefcase");
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn a_silicons_allow_list_is_read_and_changed_by_account() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/silicons/si:scout/allowed-accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "silicon": {"uuid": "Sc0", "id": "si:scout", "kind": "silicon"},
            "items": [{"account": {"uuid": "b0b", "id": "c:bob", "kind": "carbon"},
                       "allowed_at": "2026-10-10T10:00:00Z"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/silicons/si:scout/allowed-accounts/c:bob"))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/silicons/si:scout/allowed-accounts/c:bob"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let allowed = client.allowed_accounts("si:scout").await.unwrap();
    assert_eq!(allowed.items[0].account.display_id(), "c:bob");
    client.allow_account("si:scout", "c:bob").await.unwrap();
    client.disallow_account("si:scout", "c:bob").await.unwrap();
}

#[tokio::test]
async fn usage_storage_and_the_bin_name_the_drive_they_are_for() {
    let server = MockServer::start().await;
    let client = connected(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/usage"))
        .and(query_param("drive", "si:scout"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "storage": {"used_bytes": 1, "limit_bytes": 2, "remaining_bytes": 1},
            "daily_uploads": {"used_bytes": 0, "limit_bytes": 2, "remaining_bytes": 2,
                              "resets_at": "2026-10-11T00:00:00Z"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bin"))
        .and(query_param("drive", "si:scout"))
        .and(query_param("limit", "5"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/storage/configuration"))
        .and(header("idempotency-key", "storage-attempt-1"))
        .and(body_string_contains("\"drive\":\"si:scout\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "failed", "tested_at": "2026-10-10T10:00:00Z", "failure_reason": "assume_role"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let usage = client.usage(Some("si:scout")).await.unwrap();
    assert_eq!(usage.storage.remaining_bytes, 1);
    client.bin(Some("si:scout"), None, Some(5)).await.unwrap();
    let status = client
        .configure_storage_with_key(
            &briefcase_client::BucketConfiguration {
                drive: Some("si:scout".to_owned()),
                bucket_name: "scout-files".to_owned(),
                region: "us-east-1".to_owned(),
                role_arn: "arn:aws:iam::123456789012:role/briefcase".to_owned(),
                prefix: "briefcase".to_owned(),
                aws_account_id: "123456789012".to_owned(),
                encryption_mode: briefcase_client::EncryptionMode::SseS3,
                kms_key_arn: None,
            },
            &IdempotencyKey::new("storage-attempt-1").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        status.status,
        briefcase_client::BucketConfigurationState::Failed
    );
}

#[tokio::test]
async fn delegated_calls_carry_only_the_proof() {
    use briefcase_client::{
        DelegatedCreateFolder, DelegatedListEntries, DelegatedOperation, Proof,
    };
    let server = MockServer::start().await;
    let client = Client::new_unchecked(
        Config::new(&base(&server))
            .unwrap()
            .with_token("account-bearer-never-sent"),
    )
    .unwrap();
    let folder = DelegatedCreateFolder {
        operation_id: uuid::Uuid::new_v4(),
        parent_path: String::new(),
        name: "recordings".to_owned(),
    };
    let manifest = folder.prepare().unwrap();
    assert_eq!(manifest.scope(), "briefcase.folders.create");
    assert_eq!(DelegatedCreateFolder::SCOPE, "briefcase.folders.create");
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/folders/create"))
        .and(header("authorization", "Proof sap_waveform_proof"))
        .and(body_json(serde_json::to_value(&folder).unwrap()))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .expect(1)
        .mount(&server)
        .await;
    client
        .create_folder_on_behalf_of(Proof::new("sap_waveform_proof").unwrap(), &manifest)
        .await
        .unwrap();

    // A first-party app may name another drive to list.
    let listing = DelegatedListEntries {
        drive: Some("c:ada".to_owned()),
        ..DelegatedListEntries::default()
    };
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/entries/list"))
        .and(body_json(
            json!({"parent_id": null, "path": null, "drive": "c:ada",
                              "filter": null, "cursor": null, "limit": null}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    client
        .list_entries_on_behalf_of(
            Proof::new("sap_interface_proof").unwrap(),
            &listing.prepare().unwrap(),
        )
        .await
        .unwrap();

    for request in server.received_requests().await.unwrap() {
        let authorization = request.headers["authorization"].to_str().unwrap();
        assert!(authorization.starts_with("Proof sap_"), "{authorization}");
        for retired in ["x-org-id", "x-app-id", "x-iam-obo-access-token"] {
            assert!(!request.headers.contains_key(retired), "{retired}");
        }
    }
}

#[test]
fn only_a_user_verification_proof_is_accepted_and_never_echoed() {
    use briefcase_client::Proof;
    assert!(Proof::new("sap_abc").is_ok());
    for value in [
        "sapr_refresh_secret",
        "eyJ.access_secret",
        "oba_old_secret",
        "sap_",
        "",
        "sap_a b_secret",
    ] {
        let error = Proof::new(value).unwrap_err().to_string();
        // A bare prefix is fine to name; a secret never is.
        assert!(
            value.len() <= "sap_".len() || !error.contains(value),
            "{value}: {error}"
        );
    }
    assert!(
        Proof::new("sapr_refresh_secret")
            .unwrap_err()
            .to_string()
            .contains("proof refresh token")
    );
    assert_eq!(
        format!("{:?}", Proof::new("sap_secret").unwrap()),
        "Proof(<redacted>)"
    );
}

#[tokio::test]
async fn a_refused_delegated_call_is_never_retried() {
    use briefcase_client::{DelegatedLinkAccess, DelegatedManifest, Proof};
    let server = MockServer::start().await;
    let client = Client::new_unchecked(Config::new(&base(&server)).unwrap()).unwrap();
    let intent = DelegatedLinkAccess {
        operation_id: uuid::Uuid::new_v4(),
        entry_id: uuid::Uuid::new_v4(),
        enabled: true,
        expires_in_minutes: None,
    };
    let manifest = DelegatedManifest::new(&intent).unwrap();
    assert_eq!(manifest.scope(), "briefcase.link_access.update");
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/link-access"))
        .and(header("authorization", "Proof sap_link_proof"))
        .and(body_json(serde_json::to_value(&intent).unwrap()))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": {
            "code": "proof_scope_missing",
            "message": "The proof does not carry briefcase.link_access.update."
        }})))
        .expect(1)
        .mount(&server)
        .await;
    let error = client
        .set_link_access_on_behalf_of(Proof::new("sap_link_proof").unwrap(), &manifest)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("proof_scope_missing"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_byte_transfer_carries_only_its_capability() {
    use briefcase_client::{UploadCapability, UploadSource};
    let server = MockServer::start().await;
    let client = Client::new_unchecked(
        Config::new(&base(&server))
            .unwrap()
            .with_token("account-bearer-never-sent"),
    )
    .unwrap();
    let upload_id = uuid::Uuid::new_v4();
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/obo/uploads/{upload_id}/content")))
        .and(header("x-briefcase-upload-capability", "cap_secret"))
        .and(header("content-length", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "operation_id": uuid::Uuid::new_v4(), "upload_id": upload_id, "state": "staged",
            "expires_at": "2026-10-10T11:00:00Z", "published_entry_id": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let status = client
        .transfer_delegated_upload(
            upload_id,
            UploadCapability::new("cap_secret").unwrap(),
            &UploadSource::Bytes(b"bytes".to_vec()),
        )
        .await
        .unwrap();
    assert_eq!(status.state, briefcase_client::DelegatedUploadState::Staged);
    let request = &server.received_requests().await.unwrap()[0];
    assert!(!request.headers.contains_key("authorization"));
    assert!(!request.headers.contains_key("x-org-id"));
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one sharing flow verifies authenticated writes and anonymous reads"
)]
async fn shares_go_to_accounts_and_link_reads_never_carry_the_bearer() {
    use briefcase_client::{ByteRange, Invite, Recipient};
    let server = MockServer::start().await;
    let id = uuid::Uuid::new_v4();
    let next = uuid::Uuid::new_v4().to_string();
    let client = Client::new_unchecked(
        Config::new(&base(&server))
            .unwrap()
            .with_token("private-bearer"),
    )
    .unwrap();
    let invite = Invite {
        principal: Recipient::account("si:scout").unwrap(),
        access: vec![AccessRight::Read, AccessRight::Update],
        inherit: true,
        expires_in_minutes: None,
    };
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/entries/{id}/invitations")))
        .and(header("authorization", "Bearer private-bearer"))
        .and(header("idempotency-key", "share-attempt-1"))
        .and(body_json(
            json!({"principal": {"type": "silicon", "id": "si:scout"},
                              "access": ["read", "update"], "inherit": true}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": id, "principal": {"type": "silicon", "id": "Sc0"},
            "access": ["read", "update"], "inherit": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    let invitation = client
        .invite(
            id,
            &invite,
            &IdempotencyKey::new("share-attempt-1").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invitation.invitation.principal.id(), "Sc0");

    Mock::given(method("GET"))
        .and(path(format!("/api/v1/entries/{id}/invitations")))
        .and(query_param("cursor", &next))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"items": [], "next_cursor": null})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert!(
        client
            .invitations(id, Some(&next))
            .await
            .unwrap()
            .items
            .is_empty()
    );

    Mock::given(method("GET"))
        .and(path("/api/v1/public/c:ada/talks/movie.mp4"))
        .and(query_param("view", "inline"))
        .and(header("range", "bytes=4-7"))
        .respond_with(
            ResponseTemplate::new(206)
                .insert_header("Content-Range", "bytes 4-7/12")
                .set_body_bytes(b"data".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut content = client
        .public_content("c:ada", "talks/movie.mp4", Some(ByteRange::inclusive(4, 7)))
        .await
        .unwrap();
    assert_eq!(content.content_range(), Some("bytes 4-7/12"));
    assert_eq!(content.chunk().await.unwrap(), Some(b"data".to_vec()));

    // A link under an id the owner gave up moves to the owner's uuid.
    Mock::given(method("GET"))
        .and(path("/api/v1/public/c:old-ada/talks"))
        .respond_with(ResponseTemplate::new(308).set_body_json(json!({
            "moved_to": format!("{}/api/v1/public/c:ada/talks", server.uri()),
            "permanent_url": "https://briefcase.example/c:ada/talks",
            "owner": {"uuid": "zQo", "id": "c:ada"}
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/public/zQo/talks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": id, "name": "talks", "path": "talks", "entry_type": "folder",
            "content_type": null, "size": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .public_entry("c:old-ada", "talks")
            .await
            .unwrap()
            .name,
        "talks"
    );

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .filter(|request| request.url.path().starts_with("/api/v1/public/"))
            .all(|request| !request.headers.contains_key("authorization")),
        "link reads must never leak a configured bearer"
    );
}

#[tokio::test]
#[allow(deprecated)]
async fn every_request_opts_out_of_telemetry_and_nothing_is_ever_sent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id": "briefcase", "accounts_url": "https://accounts.example", "delegated_scopes": []
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "not_found", "message": "The requested resource was not found."}
        })))
        .mount(&server)
        .await;
    // The retired switches and submit are accepted and do nothing.
    let event = briefcase_client::telemetry::Event::new(
        briefcase_client::telemetry::Source::Sdk,
        "upload",
        briefcase_client::telemetry::Stage::Completed,
    );
    briefcase_client::telemetry::submit(&base(&server), &event)
        .await
        .unwrap();
    assert!(!briefcase_client::telemetry::enabled_from_env());
    for enabled in [true, false] {
        let config = Config::new(&base(&server))
            .unwrap()
            .with_telemetry(enabled)
            .with_telemetry_source(briefcase_client::telemetry::Source::Cli);
        let anonymous = Client::new_unchecked(config.clone()).unwrap();
        anonymous.accounts_info().await.unwrap();
        let signed_in = Client::new_unchecked(config.with_token("access")).unwrap();
        let entry = uuid::Uuid::parse_str(ENTRY_ID).unwrap();
        assert!(signed_in.entry(entry).await.is_err());
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert_ne!(request.url.path(), "/api/v1/telemetry");
        assert_eq!(request.headers["x-briefcase-telemetry"], "off");
        assert!(!request.headers.contains_key("x-briefcase-source"));
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn expiring_shares_and_self_destruct_use_the_contracted_shapes() {
    use briefcase_client::{ExpiryChange, Invite, Recipient};
    let server = MockServer::start().await;
    let id = uuid::Uuid::new_v4();
    let grant = uuid::Uuid::new_v4();
    let client = Client::new_unchecked(
        Config::new(&base(&server))
            .unwrap()
            .with_token("private-bearer"),
    )
    .unwrap();
    let invitation = json!({"id": grant, "principal": {"type": "carbon", "id": "a1ex"},
        "access": ["read"], "inherit": true, "expires_at": "2026-10-25T10:00:00Z"});

    Mock::given(method("POST"))
        .and(path(format!("/api/v1/entries/{id}/invitations")))
        .and(body_json(
            json!({"principal": {"type": "carbon", "id": "c:alex"},
            "access": ["read"], "inherit": true, "expires_in_minutes": 90}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(invitation.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let created = client
        .invite(
            id,
            &Invite {
                principal: Recipient::Carbon("c:alex".into()),
                access: vec![AccessRight::Read],
                inherit: true,
                expires_in_minutes: Some(90),
            },
            &IdempotencyKey::new("expiring-invitation").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.expires_at.as_deref(), Some("2026-10-25T10:00:00Z"));

    for (change, body) in [
        (
            ExpiryChange::ExpireIn(15),
            json!({"expires_in_minutes": 15}),
        ),
        (ExpiryChange::Permanent, json!({"permanent": true})),
    ] {
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/entries/{id}/invitations/{grant}")))
            .and(header("idempotency-key", "expiring-change"))
            .and(body_json(body))
            .respond_with(ResponseTemplate::new(200).set_body_json(invitation.clone()))
            .expect(1)
            .mount(&server)
            .await;
        client
            .change_expiring_share(
                id,
                grant,
                change,
                &IdempotencyKey::new("expiring-change").unwrap(),
            )
            .await
            .unwrap();
    }

    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/entries/{id}/link-access")))
        .and(body_json(
            json!({"enabled": true, "expires_in_minutes": 60}),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"can_manage": true,
            "enabled": true, "effective": true, "inherited_from": null, "url": null,
            "expires_at": "2026-10-25T11:00:00Z"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let link = client
        .set_expiring_link_access(id, 60, &IdempotencyKey::new("expiring-link").unwrap())
        .await
        .unwrap();
    assert_eq!(link.expires_at.as_deref(), Some("2026-10-25T11:00:00Z"));

    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/entries/{id}/self-destruct")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    client.make_permanent(id).await.unwrap();

    let mut document = entry_document();
    document["self_destruct_at"] = json!("2026-10-25T12:00:00Z");
    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains("name=\"self_destruct_minutes\""))
        .respond_with(ResponseTemplate::new(201).set_body_json(document))
        .expect(1)
        .mount(&server)
        .await;
    let entry = client
        .upload(
            &Upload::bytes(Destination::path("notes"), "note.txt", b"hi".to_vec())
                .self_destructing(30),
        )
        .await
        .unwrap();
    assert!(entry.self_destruct_at.is_some());
}
