//! Process-level checks that credentials never cross a saved trust boundary.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, body_string_contains, header, method, path, query_param},
};

const ACTOR_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f803";
const TEST_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f800";
const ROOT_KEY: &str = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6";
const ENTRY_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f828";
const DESTINATION_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f829";

fn scope(server: &MockServer, organization: &str) -> Value {
    json!({
        "deployment_origin": format!("{}/", server.uri()),
        "organization": organization,
    })
}

fn session(expires_at: &str) -> Value {
    json!({
        "access_token": "stored-access-must-not-leak",
        "refresh_token": "stored-refresh-must-not-leak",
        "expires_at": expires_at,
        "actor": {
            "principal_id": ACTOR_ID,
            "type": "carbon",
            "public_id": "cos:tester",
        },
        "org_id": "tos",
    })
}

fn version_document() -> Value {
    let operations: Vec<Value> = briefcase_client::OPERATIONS
        .iter()
        .map(|operation| {
            json!({
                "id": operation.id,
                "version": operation.version,
                "method": operation.method,
                "path": operation.path,
            })
        })
        .collect();
    json!({
        "service": "silicon-briefcase",
        "selected_api_version": "v1",
        "supported_api_versions": ["v1"],
        "contract_version": "test",
        "build": "test",
        "operations": operations,
    })
}

fn entry_document() -> Value {
    json!({
        "id": ENTRY_ID,
        "org_id": "tos",
        "type": "file",
        "visibility": "full",
        "name": "note.txt",
        "path": "private/cos:tester/apps/tos>notes/note.txt",
        "parent_id": null,
        "root_type": "private",
        "tag": null,
        "content_type": "text/plain",
        "size": 4,
        "render": "document",
        "permanent_url": "https://briefcase.example/org/tos/private/cos:tester/apps/tos%3Enotes/note.txt",
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": "cos:tester"},
        "origin_app_id": "tos>notes",
        "effective_access": ["read", "update"],
        "created_at": "2026-09-04T00:00:00Z",
        "updated_at": "2026-09-04T00:00:00Z",
        "deleted_at": null,
    })
}

fn fingerprint(intent: &Value) -> String {
    let body = serde_json::to_vec(intent).unwrap();
    format!("{:x}", Sha256::digest(body))
}

fn write_state(home: &Path, server: &MockServer, credentials: &Value) {
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "auto_update": false,
            "current_profile": "work",
            "profiles": {
                "work": {
                    "url": format!("{}/api/v1/", server.uri()),
                    "org": "tos",
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        home.join("credentials.json"),
        serde_json::to_vec_pretty(credentials).unwrap(),
    )
    .unwrap();
}

async fn briefcase(home: &Path, arguments: &[String]) -> std::process::Output {
    let executable = env!("CARGO_BIN_EXE_briefcase").to_owned();
    let home = home.to_owned();
    let arguments = arguments.to_owned();
    tokio::task::spawn_blocking(move || {
        Command::new(executable)
            .args(arguments)
            .env("BRIEFCASE_HOME", home)
            .env("BRIEFCASE_AUTO_UPDATE", "off")
            .output()
            .expect("the test CLI must run")
    })
    .await
    .expect("the CLI process must join")
}

async fn briefcase_with_stdin(
    home: &Path,
    arguments: &[String],
    input: &[u8],
) -> std::process::Output {
    let executable = env!("CARGO_BIN_EXE_briefcase").to_owned();
    let home = home.to_owned();
    let arguments = arguments.to_owned();
    let input = input.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(executable)
            .args(arguments)
            .env("BRIEFCASE_HOME", home)
            .env("BRIEFCASE_AUTO_UPDATE", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the test CLI must start");
        child
            .stdin
            .take()
            .expect("the test CLI must have piped stdin")
            .write_all(&input)
            .expect("the proof must be written");
        child.wait_with_output().expect("the test CLI must finish")
    })
    .await
    .expect("the CLI process must join")
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_bearers_never_follow_url_or_organization_overrides() {
    let saved = MockServer::start().await;
    let attacker = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &saved,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&saved, "tos")},
        }),
    );

    let output = briefcase(
        home.path(),
        &[
            "--url".into(),
            format!("{}/api/v1/", attacker.uri()),
            "ls".into(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to send"));

    let output = briefcase(home.path(), &["--org".into(), "other".into(), "ls".into()]).await;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to send"));
    assert!(
        attacker
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    assert!(
        saved
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_session_uses_its_own_binding_not_the_production_binding() {
    let test_deployment = MockServer::start().await;
    let production_deployment = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &test_deployment,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "test_sessions": {
                "work": {(TEST_ID): session("2099-01-01T00:00:00Z")}
            },
            "testing_environment_keys": {"work": {(TEST_ID): ROOT_KEY}},
            "production_credential_scopes": {
                "work": scope(&production_deployment, "tos")
            },
            "testing_environment_scopes": {
                "work": {(TEST_ID): scope(&test_deployment, "tos")}
            },
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(header(
            "authorization",
            "Bearer stored-access-must-not-leak",
        ))
        .and(header("x-testing-environment-key", ROOT_KEY))
        .and(header("x-org-id", "tos"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "next_cursor": null,
        })))
        .mount(&test_deployment)
        .await;

    let output = briefcase(
        home.path(),
        &[
            "--test".into(),
            TEST_ID.into(),
            "--no-verify".into(),
            "ls".into(),
        ],
    )
    .await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        test_deployment
            .received_requests()
            .await
            .unwrap_or_default()
            .len(),
        1
    );
    assert!(
        production_deployment
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_login_never_sends_a_stored_root_to_an_overridden_url() {
    let saved = MockServer::start().await;
    let attacker = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &saved,
        &json!({
            "testing_environment_keys": {"work": {(TEST_ID): ROOT_KEY}},
            "testing_environment_scopes": {"work": {(TEST_ID): scope(&saved, "tos")}},
        }),
    );

    let output = briefcase(
        home.path(),
        &[
            "--url".into(),
            format!("{}/api/v1/", attacker.uri()),
            "--org".into(),
            "tos".into(),
            "--test".into(),
            TEST_ID.into(),
            "login".into(),
            "--slt".into(),
            "slt-must-not-cross-origins".into(),
        ],
    )
    .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to send"));
    assert!(
        attacker
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn login_never_persists_an_organization_unbound_session() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/slt"))
        .and(body_json(json!({"slt": "unscoped-slt"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "unbound-access-must-not-persist",
            "refresh_token": "unbound-refresh-must-not-persist",
            "token_type": "Bearer",
            "expires_in": 900,
            "scope": "briefcase",
            "actor": {
                "principal_id": ACTOR_ID,
                "type": "carbon",
                "public_id": "cos:tester"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let output = briefcase_with_stdin(
        home.path(),
        &[
            "--url".into(),
            format!("{}/api/v1/", server.uri()),
            "--org".into(),
            "tos".into(),
            "--no-verify".into(),
            "login".into(),
            "--slt-stdin".into(),
        ],
        b"unscoped-slt\n",
    )
    .await;

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("organization-unbound"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("signed in"));
    assert!(!home.path().join("config.json").exists());
    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    assert_eq!(credentials["sessions"], json!({}));
    assert_eq!(credentials["test_sessions"], json!({}));
    assert_eq!(credentials["tokens"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn entry_pages_are_resumable_and_retain_the_next_cursor() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&server, "tos")},
        }),
    );
    let response = json!({
        "items": [entry_document()],
        "next_cursor": "page-three",
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(header(
            "authorization",
            "Bearer stored-access-must-not-leak",
        ))
        .and(query_param("cursor", "page-two"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(2)
        .mount(&server)
        .await;

    let human = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "ls".into(),
            "--cursor".into(),
            "page-two".into(),
            "--limit".into(),
            "1".into(),
        ],
    )
    .await;
    assert!(human.status.success());
    assert!(String::from_utf8_lossy(&human.stderr).contains("--cursor page-three"));

    let json_output = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "--json".into(),
            "ls".into(),
            "--cursor".into(),
            "page-two".into(),
            "--limit".into(),
            "1".into(),
        ],
    )
    .await;
    assert!(json_output.status.success());
    let page: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], "page-three");

    Mock::given(method("GET"))
        .and(path("/api/v1/bin"))
        .and(query_param("cursor", "bin-two"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [entry_document()],
            "next_cursor": "bin-three",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let bin = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "--json".into(),
            "bin".into(),
            "list".into(),
            "--cursor".into(),
            "bin-two".into(),
        ],
    )
    .await;
    assert!(bin.status.success());
    let page: Value = serde_json::from_slice(&bin.stdout).unwrap();
    assert_eq!(page["next_cursor"], "bin-three");
}

#[tokio::test(flavor = "multi_thread")]
async fn all_entry_pages_reach_exhaustion_and_reject_cursor_cycles() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&server, "tos")},
        }),
    );
    let mut second_entry = entry_document();
    second_entry["id"] = json!("01a067ce-7f19-7790-820a-0be6b3d4f829");
    second_entry["name"] = json!("second.txt");
    second_entry["path"] = json!("private/cos:tester/apps/tos>notes/second.txt");
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(query_param("cursor", "page-one"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [entry_document()],
            "next_cursor": "page-two",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(query_param("cursor", "page-two"))
        .and(query_param("limit", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [second_entry],
            "next_cursor": null,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let output = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "--json".into(),
            "ls".into(),
            "--cursor".into(),
            "page-one".into(),
            "--limit".into(),
            "1".into(),
            "--all".into(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let page: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["next_cursor"].is_null());

    let looping_server = MockServer::start().await;
    let looping_home = tempfile::tempdir().unwrap();
    write_state(
        looping_home.path(),
        &looping_server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&looping_server, "tos")},
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/entries"))
        .and(query_param("cursor", "repeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [entry_document()],
            "next_cursor": "repeat",
        })))
        .expect(1)
        .mount(&looping_server)
        .await;
    let output = briefcase(
        looping_home.path(),
        &[
            "--no-verify".into(),
            "ls".into(),
            "--cursor".into(),
            "repeat".into(),
            "--all".into(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("repeated a pagination cursor"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_path_access_request_uses_one_direct_privacy_preserving_route() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let hidden_path = "private/cos:owner/hidden/report.pdf";
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&server, "tos")},
        }),
    );
    Mock::given(method("POST"))
        .and(path("/api/v1/access-requests"))
        .and(header(
            "authorization",
            "Bearer stored-access-must-not-leak",
        ))
        .and(header("x-org-id", "tos"))
        .and(body_json(json!({
            "path": hidden_path,
            "access": ["read", "update"],
            "reason": "quarterly review"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "01a067ce-7f19-7790-820a-0be6b3d4f850",
            "entry_id": ENTRY_ID,
            "requested_by": {"type": "carbon", "id": "cos:tester"},
            "access": ["read", "update"],
            "status": "pending",
            "created_at": "2026-09-04T00:00:00Z"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let output = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "request".into(),
            hidden_path.into(),
            "--access".into(),
            "read,update".into(),
            "--reason".into(),
            "quarterly review".into(),
        ],
    )
    .await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].url.path(), "/api/v1/access-requests");
    assert!(requests[0].headers.contains_key("idempotency-key"));
    assert!(!requests[0].url.path().contains("/entries/"));
    assert!(!requests[0].url.path().contains("/org/"));

    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    assert_eq!(credentials["pending_mutations"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_verifies_the_contract_before_presenting_the_rotating_token() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": {"work": session("2000-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&server, "tos")},
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(json!({
                    "service": "silicon-briefcase",
                    "selected_api_version": "v1",
                    "supported_api_versions": ["v1"],
                    "contract_version": "incompatible-test",
                    "build": "test",
                    "operations": [],
                })),
        )
        .mount(&server)
        .await;

    let output = briefcase(home.path(), &["ls".into()]).await;
    assert_eq!(output.status.code(), Some(1));

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/api/version");
    assert!(!requests[0].headers.contains_key("authorization"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("stored-refresh-must-not-leak"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_obo_upload_never_loads_or_refreshes_an_invalid_stored_member_session() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join("note.txt");
    std::fs::write(&file, b"note").unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            // This deliberately cannot deserialize as `StoredSession`. A
            // production OBO invocation has no reason to load it at all.
            "sessions": {"work": {
                "access_token": ["invalid"],
                "refresh_token": {"invalid": true},
                "expires_at": "2000-01-01T00:00:00Z"
            }},
            "production_credential_scopes": {"work": scope(&server, "tos")},
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/obo/files"))
        .and(header("x-org-id", "tos"))
        .and(header("x-app-id", "tos>notes"))
        .and(header("x-iam-obo-access-proof", "proof-once"))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    let output = briefcase_with_stdin(
        home.path(),
        &[
            "app".into(),
            "upload".into(),
            "--app-id".into(),
            "tos>notes".into(),
            "--proof-stdin".into(),
            file.display().to_string(),
        ],
        b"proof-once\n",
    )
    .await;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| !request.headers.contains_key("authorization"))
    );
    assert!(
        requests
            .iter()
            .all(|request| request.url.path() != "/api/v1/auth/refresh")
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one crash-recovery scenario covers all three path-addressed mutation shapes"
)]
async fn durable_path_mutations_replay_with_persisted_ids_and_no_path_lookups() {
    let server = MockServer::start().await;
    let home = tempfile::tempdir().unwrap();
    let upload_file = home.path().join("upload.txt");
    std::fs::write(&upload_file, b"stable upload").unwrap();
    let canonical_upload = std::fs::canonicalize(&upload_file)
        .unwrap()
        .display()
        .to_string();
    let api_url = format!("{}/api/v1/", server.uri());
    let source = "private/cos:tos/source.txt";
    let parent = "private/cos:tos/archive";
    let move_destination = format!("{parent}/moved.txt");
    let mkdir_path = format!("{parent}/new-folder");

    let move_address = fingerprint(&json!({
        "target": source,
        "destination": move_destination,
        "testing_environment_id": null,
    }));
    let move_fingerprint = fingerprint(&json!({
        "operation": "update-entry",
        "profile": "work",
        "url": api_url,
        "org": "tos",
        "testing_environment_id": null,
        "target": source,
        "destination": move_destination,
    }));
    let mkdir_address = fingerprint(&json!({
        "path": mkdir_path,
        "testing_environment_id": null,
    }));
    let mkdir_fingerprint = fingerprint(&json!({
        "operation": "create-folder",
        "profile": "work",
        "url": api_url,
        "org": "tos",
        "testing_environment_id": null,
        "name": "new-folder",
        "parent": format!("path:{parent}"),
        "root_type": null,
        "tag": null,
        "invitees": [],
    }));
    let upload_destination = format!("path:{parent}");
    let upload_address = fingerprint(&json!({
        "source": canonical_upload,
        "destination": upload_destination,
        "file_name": "upload.txt",
        "testing_environment_id": null,
    }));
    let upload_fingerprint = fingerprint(&json!({
        "operation": "upload-file",
        "profile": "work",
        "url": api_url,
        "org": "tos",
        "testing_environment_id": null,
        "source": canonical_upload,
        "destination": upload_destination,
        "file_name": "upload.txt",
        "content_type": briefcase_client::guess_content_type("upload.txt"),
        "content_sha256": format!("{:x}", Sha256::digest(b"stable upload")),
    }));

    let move_scope = format!("entry:move:work:production:{move_address}");
    let mkdir_scope = format!("entry:mkdir:work:production:{mkdir_address}");
    let upload_scope = format!("entry:put:work:production:{upload_address}");
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(&server, "tos")},
            "pending_mutations": {
                (move_scope): {
                    "idempotency_key": "move-attempt-0001",
                    "request_fingerprint": move_fingerprint,
                    "resource_id": ENTRY_ID,
                    "destination_id": DESTINATION_ID,
                },
                (mkdir_scope): {
                    "idempotency_key": "mkdir-attempt-0001",
                    "request_fingerprint": mkdir_fingerprint,
                    "destination_id": DESTINATION_ID,
                },
                (upload_scope): {
                    "idempotency_key": "upload-attempt-0001",
                    "request_fingerprint": upload_fingerprint,
                    "destination_id": DESTINATION_ID,
                },
            },
        }),
    );

    Mock::given(method("PATCH"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}")))
        .and(header("idempotency-key", "move-attempt-0001"))
        .and(body_json(json!({
            "name": "moved.txt",
            "parent_id": DESTINATION_ID,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(entry_document()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/entries"))
        .and(header("idempotency-key", "mkdir-attempt-0001"))
        .and(body_json(json!({
            "name": "new-folder",
            "parent_id": DESTINATION_ID,
            "invitees": [],
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(header("idempotency-key", "upload-attempt-0001"))
        .and(body_string_contains("name=\"parent_id\""))
        .and(body_string_contains(DESTINATION_ID))
        .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
        .mount(&server)
        .await;

    for arguments in [
        vec![
            "--no-verify".into(),
            "mv".into(),
            source.into(),
            move_destination,
        ],
        vec!["--no-verify".into(), "mkdir".into(), mkdir_path],
        vec![
            "--no-verify".into(),
            "put".into(),
            upload_file.display().to_string(),
            parent.into(),
        ],
    ] {
        let output = briefcase(home.path(), &arguments).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 3);
    assert!(
        requests
            .iter()
            .all(|request| request.method.as_str() != "GET")
    );
    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    assert_eq!(credentials["pending_mutations"], json!({}));
}
