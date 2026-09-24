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

// Most command fixtures start with an active saved login. Individual tests
// install higher-priority status responses when exercising invalidation.
async fn authenticated_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authenticated": true, "actor": null, "organizations": ["tos"], "expires_at": null
        })))
        .with_priority(10)
        .mount(&server)
        .await;
    server
}

const ACTOR_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f803";
const TEST_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f800";
const ROOT_KEY: &str = "ask_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
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
        "path": "apps/notes/private/cos:tester/note.txt",
        "parent_id": null,
        "root_type": "private",
        "tag": null,
        "content_type": "text/plain",
        "size": 4,
        "render": "document",
        "permanent_url": "https://briefcase.example/org/tos/apps/notes/private/cos:tester/note.txt",
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": "cos:tester"},
        "origin_app_id": "notes",
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
            .env("BRIEFCASE_TELEMETRY", "off")
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
            .env("BRIEFCASE_TELEMETRY", "off")
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
    let saved = authenticated_server().await;
    let attacker = authenticated_server().await;
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
    let test_deployment = authenticated_server().await;
    let production_deployment = authenticated_server().await;
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
        .and(header("x-briefcase-app-secret", ROOT_KEY))
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
        2
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
    let saved = authenticated_server().await;
    let attacker = authenticated_server().await;
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
async fn test_actor_login_sends_the_id_and_preserves_the_production_session() {
    for (actor_id, actor_type) in [("alice", "carbon"), ("worker:tos", "silicon")] {
        let server = authenticated_server().await;
        let home = tempfile::tempdir().unwrap();
        let mut production = session("2099-01-01T00:00:00Z");
        production["organizations"] = json!(["tos"]);
        write_state(
            home.path(),
            &server,
            &json!({
                "sessions": {"work": production},
                "testing_environment_keys": {"work": {(TEST_ID): ROOT_KEY}},
                "testing_environment_scopes": {"work": {(TEST_ID): scope(&server, "tos")}},
            }),
        );
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/slt"))
            .and(header("x-briefcase-app-secret", ROOT_KEY))
            .and(body_json(json!({"slt": actor_id})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "test-access",
                "refresh_token": "test-refresh",
                "token_type": "Bearer",
                "expires_in": 1800,
                "scope": "briefcase",
                "actor": {"principal_id": ACTOR_ID, "type": actor_type, "public_id": actor_id},
                "organizations": ["tos"],
            })))
            .expect(1)
            .mount(&server)
            .await;
        let output = briefcase(
            home.path(),
            &[
                "--test".into(),
                TEST_ID.into(),
                "--no-verify".into(),
                "--json".into(),
                "login".into(),
                actor_id.into(),
            ],
        )
        .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["actor"]["public_id"], actor_id);
        assert_eq!(result["test_environment_id"], TEST_ID);
        assert!(String::from_utf8_lossy(&output.stderr).contains("TEST ENVIRONMENT"));
        let credentials: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
                .unwrap();
        assert_eq!(credentials["sessions"]["work"], production);
        assert_eq!(
            credentials["test_sessions"]["work"][TEST_ID]["access_token"],
            "test-access"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].headers.contains_key("authorization"));
        assert!(requests[0].headers.contains_key("idempotency-key"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unscoped_login_does_not_turn_the_workspace_preference_into_a_grant() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/slt"))
        .and(body_json(json!({"slt": "unscoped-slt"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "unscoped-access",
            "refresh_token": "unscoped-refresh",
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

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    assert_eq!(
        credentials["sessions"]["default"]["access_token"],
        "unscoped-access"
    );
    assert!(credentials["sessions"]["default"]["org_id"].is_null());
    assert_eq!(
        credentials["sessions"]["default"]["organizations"],
        json!([])
    );
    assert_eq!(
        credentials["production_credential_scopes"]["default"]["organization"],
        ""
    );
    assert_eq!(credentials["test_sessions"], json!({}));
    assert_eq!(credentials["tokens"], json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn entry_pages_are_resumable_and_retain_the_next_cursor() {
    let server = authenticated_server().await;
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
    let server = authenticated_server().await;
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
    second_entry["path"] = json!("apps/notes/private/cos:tester/second.txt");
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

    let looping_server = authenticated_server().await;
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
async fn refresh_verifies_the_contract_before_presenting_the_rotating_token() {
    let server = authenticated_server().await;
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
    let server = authenticated_server().await;
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
        .and(header("x-app-id", "notes"))
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
            "notes".into(),
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
    let server = authenticated_server().await;
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

    let requests: Vec<_> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path() != "/api/v1/auth/status")
        .collect();
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

#[tokio::test]
async fn iam_discovery_does_not_read_or_send_member_credentials() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(home.path(), &server, &json!({}));
    std::fs::write(home.path().join("credentials.json"), "not valid JSON").unwrap();
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/iam"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id": "briefcase", "test_environment_id": null, "iam_environment_id": null
        })))
        .expect(1)
        .mount(&server)
        .await;
    let output = briefcase(home.path(), &["iam".into(), "--json".into()]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["app_id"], "briefcase");
    for request in server.received_requests().await.unwrap() {
        assert!(request.headers.get("authorization").is_none());
    }
}

#[tokio::test]
async fn login_status_without_credentials_is_json_and_needs_no_network() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(home.path(), &server, &json!({}));
    let output = briefcase(
        home.path(),
        &["login".into(), "status".into(), "--json".into()],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["authenticated"], false);
    assert!(value["actor"].is_null());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn login_status_checks_override_identity_instead_of_the_saved_actor() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": { "work": session("2020-01-01T00:00:00Z") },
            "production_credential_scopes": { "work": scope(&server, "tos") }
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
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .and(header("authorization", "Bearer override-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authenticated": true,
            "actor": { "principal_id": ACTOR_ID, "type": "silicon", "public_id": "agent-a" },
            "organizations": ["tos", "other"], "expires_at": 4_102_444_800_i64
        })))
        .expect(1)
        .mount(&server)
        .await;
    let output = briefcase(
        home.path(),
        &[
            "login".into(),
            "status".into(),
            "--json".into(),
            "--token".into(),
            "override-token".into(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["authenticated"], true);
    assert_eq!(value["actor"]["type"], "silicon");
    assert_eq!(value["actor"]["public_id"], "agent-a");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("token"));
    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == "/api/v1/auth/refresh")
    );
}

#[tokio::test]
async fn login_status_refreshes_unscoped_sessions_without_choosing_an_organization() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    let mut stored = session("2020-01-01T00:00:00Z");
    stored["org_id"] = Value::Null;
    stored["organizations"] = json!(["tos", "other"]);
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions": { "work": stored },
            "production_credential_scopes": { "work": scope(&server, "") }
        }),
    );
    let config_file = home.path().join("config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_file).unwrap()).unwrap();
    config["profiles"]["work"]["org"] = json!("");
    std::fs::write(config_file, serde_json::to_vec(&config).unwrap()).unwrap();
    Mock::given(method("GET"))
        .and(path("/api/version"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("briefcase-api-version", "v1")
                .set_body_json(version_document()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path("/api/v1/auth/refresh"))
        .and(body_json(json!({"refresh_token": "stored-refresh-must-not-leak"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "new-access", "refresh_token": "new-refresh", "token_type": "Bearer",
            "expires_in": 1800, "scope": "profile", "org_id": null, "organizations": ["tos", "other"],
            "actor": {"principal_id": ACTOR_ID, "type": "carbon", "public_id": "cos:tester"}
        }))).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .and(header("authorization", "Bearer new-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authenticated": true,
            "actor": {"principal_id": ACTOR_ID, "type": "carbon", "public_id": "cos:tester"},
            "organizations": ["tos", "other"], "expires_at": 4_102_444_800_i64
        })))
        .expect(1)
        .mount(&server)
        .await;
    let output = briefcase(
        home.path(),
        &["login".into(), "status".into(), "--json".into()],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["authenticated"], true);
    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    assert_eq!(
        credentials["sessions"]["work"]["refresh_token"],
        "new-refresh"
    );
}

#[tokio::test]
async fn delayed_or_legacy_pending_refresh_does_not_extend_replayed_access_lifetime() {
    for started in [Value::Null, json!("2000-01-01T00:00:00Z")] {
        let server = authenticated_server().await;
        let home = tempfile::tempdir().unwrap();
        let mut stored = session("2020-01-01T00:00:00Z");
        stored["refresh_idempotency_key"] = json!("original-refresh-attempt");
        stored["refresh_started_at"] = started;
        write_state(
            home.path(),
            &server,
            &json!({"sessions":{"work":stored},
            "production_credential_scopes":{"work":scope(&server,"tos")}}),
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
        for (old, new) in [
            ("stored-refresh-must-not-leak", "replayed"),
            ("replayed", "fresh"),
        ] {
            Mock::given(method("POST")).and(path("/api/v1/auth/refresh"))
                .and(body_json(json!({"refresh_token":old})))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "access_token":format!("access-{new}"),"refresh_token":new,"token_type":"Bearer",
                    "expires_in":1800,"scope":"profile","org_id":"tos","organizations":["tos"],
                    "actor":{"principal_id":ACTOR_ID,"type":"carbon","public_id":"c:tester"}})))
                .expect(1).mount(&server).await;
        }
        Mock::given(method("GET")).and(path("/api/v1/auth/status"))
            .and(header("authorization", "Bearer access-fresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"authenticated":true,
                "actor":{"principal_id":ACTOR_ID,"type":"carbon","public_id":"c:tester"},"organizations":["tos"],"expires_at":4_102_444_800_i64})))
            .expect(1).mount(&server).await;
        let output = briefcase(
            home.path(),
            &["login".into(), "status".into(), "--json".into()],
        )
        .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let credentials: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
                .unwrap();
        assert_eq!(credentials["sessions"]["work"]["refresh_token"], "fresh");
        assert!(credentials["sessions"]["work"]["refresh_started_at"].is_null());
    }
}

fn clean_cli() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_briefcase"));
    command.env("BRIEFCASE_TELEMETRY", "off");
    for key in [
        "BRIEFCASE_HOME",
        "SILICON_HOME",
        "BRIEFCASE_PROFILE",
        "BRIEFCASE_URL",
        "BRIEFCASE_ORG",
        "BRIEFCASE_TOKEN",
        "BRIEFCASE_TEST",
    ] {
        command.env_remove(key);
    }
    command.env("BRIEFCASE_AUTO_UPDATE", "off");
    command
}

#[test]
fn shared_home_precedence_and_persistent_configuration_are_isolated() {
    let os_home = tempfile::tempdir().unwrap();
    let silicon_home = tempfile::tempdir().unwrap();
    let configured = tempfile::tempdir().unwrap();
    let explicit = tempfile::tempdir().unwrap();
    let output = clean_cli()
        .env("HOME", os_home.path())
        .env("SILICON_HOME", silicon_home.path())
        .args(["config", "set", "auto-update", "off"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(silicon_home.path().join(".briefcase/config.json").is_file());
    assert!(!os_home.path().join(".briefcase").exists());
    let output = clean_cli()
        .env("HOME", os_home.path())
        .env("SILICON_HOME", silicon_home.path())
        .args(["config", "home"])
        .arg(configured.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(silicon_home.path().join(".briefcase-home").is_file());
    assert!(!os_home.path().join(".briefcase-home").exists());
    let output = clean_cli()
        .env("HOME", os_home.path())
        .env("SILICON_HOME", silicon_home.path())
        .args(["config", "set", "auto-update", "off"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(configured.path().join(".briefcase/config.json").is_file());
    let output = clean_cli()
        .env("HOME", os_home.path())
        .env("SILICON_HOME", silicon_home.path())
        .env("BRIEFCASE_HOME", explicit.path())
        .args(["config", "set", "auto-update", "off"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(explicit.path().join("config.json").is_file());
}

#[test]
fn default_home_falls_back_and_shared_home_works_without_home() {
    let home = tempfile::tempdir().unwrap();
    let output = clean_cli()
        .env("HOME", home.path())
        .args(["config", "set", "auto-update", "off"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(home.path().join(".briefcase/config.json").is_file());
    let shared = tempfile::tempdir().unwrap();
    let output = clean_cli()
        .env_remove("HOME")
        .env("SILICON_HOME", shared.path())
        .args(["config", "set", "auto-update", "off"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(shared.path().join(".briefcase/config.json").is_file());
    let output = clean_cli()
        .env("HOME", home.path())
        .env("SILICON_HOME", "")
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a directory"));
}

#[test]
fn help_is_available_without_state_and_documents_login_inspection() {
    for args in [
        vec!["--help"],
        vec!["-h"],
        vec!["login", "--help"],
        vec!["login", "status", "--help"],
    ] {
        let output = clean_cli().env_remove("HOME").args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    let output = clean_cli()
        .env_remove("HOME")
        .args(["--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("briefcase iam --json"));
    assert!(help.contains("briefcase login status --json"));
    assert!(help.contains("SILICON_HOME"));
}

#[tokio::test]
async fn failed_test_commands_keep_json_clean_and_print_the_test_footer() {
    let home = tempfile::tempdir().unwrap();
    let output = briefcase(
        home.path(),
        &[
            "--test".into(),
            TEST_ID.into(),
            "--json".into(),
            "ls".into(),
        ],
    )
    .await;
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("TEST ENVIRONMENT"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .ends_with(&format!("TEST ENVIRONMENT — {TEST_ID}\n"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn enabling_link_access_returns_file_and_folder_urls() {
    for (entry_path, json_output) in [
        ("public/Shared%20folder", false),
        ("public/Shared%20folder/file%23.txt", true),
    ] {
        let server = authenticated_server().await;
        let home = tempfile::tempdir().unwrap();
        write_state(
            home.path(),
            &server,
            &json!({
                "sessions": {"work": session("2099-01-01T00:00:00Z")},
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
        let url = format!("https://briefcase.example/org/tos/{entry_path}");
        Mock::given(method("PUT"))
            .and(path(format!("/api/v1/entries/{ENTRY_ID}/link-access")))
            .and(body_json(json!({"enabled":true})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "can_manage":true,"enabled":true,"effective":true,"inherited_from":null,"url":url
            })))
            .expect(1)
            .mount(&server)
            .await;
        let mut args = vec![
            "link".into(),
            ENTRY_ID.into(),
            "--enabled".into(),
            "true".into(),
        ];
        if json_output {
            args.push("--json".into());
        }
        let result = briefcase(home.path(), &args).await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["url"], url);
        assert_eq!(value["enabled"], true);
    }
}

#[tokio::test]
async fn bug_report_sends_only_explicit_content_and_replays_its_operation_identity() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({"sessions":{"work":session("2099-01-01T00:00:00Z")}}),
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
        .and(path("/api/v1/reports"))
        .and(header(
            "authorization",
            "Bearer stored-access-must-not-leak",
        ))
        .and(body_json(json!({"message":"Reproduction steps"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id":ENTRY_ID,"accepted":true})),
        )
        .expect(2)
        .mount(&server)
        .await;
    for _ in 0..2 {
        let result = briefcase(
            home.path(),
            &[
                "report".into(),
                "Reproduction steps".into(),
                "--json".into(),
            ],
        )
        .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["accepted"], true);
        assert!(String::from_utf8_lossy(&result.stderr).contains("--pr"));
    }
    let requests = server.received_requests().await.unwrap();
    let reports: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/api/v1/reports")
        .collect();
    assert_eq!(
        reports[0].headers["idempotency-key"],
        reports[1].headers["idempotency-key"]
    );
}

#[tokio::test]
async fn telemetry_preference_persists_and_never_attaches_command_arguments() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(home.path(), &server, &json!({}));
    Mock::given(method("POST"))
        .and(path("/api/v1/telemetry"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let state = home.path().to_owned();
    tokio::task::spawn_blocking(move || {
        for args in [
            ["config", "show"].as_slice(),
            ["config", "set", "telemetry", "off"].as_slice(),
            ["config", "show"].as_slice(),
        ] {
            let output = clean_cli()
                .env("BRIEFCASE_HOME", &state)
                .env("BRIEFCASE_TELEMETRY", "on")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    })
    .await
    .unwrap();
    let saved: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("config.json")).unwrap()).unwrap();
    assert_eq!(saved["telemetry"], false);
    let requests = server.received_requests().await.unwrap();
    let telemetry = requests
        .iter()
        .find(|r| r.url.path() == "/api/v1/telemetry")
        .unwrap();
    let event: Value = serde_json::from_slice(&telemetry.body).unwrap();
    assert_eq!(event["operation"], "config");
    assert_eq!(event["source"], "cli");
    assert!(event["duration_ms"].is_number());
    assert!(!String::from_utf8_lossy(&telemetry.body).contains("profiles"));
    assert!(!telemetry.headers.contains_key("authorization"));
}

#[test]
fn legacy_updater_settings_cannot_reenable_briefcase_updates() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".briefcase");
    std::fs::create_dir_all(&state).unwrap();
    let config_path = state.join("config.json");
    let original = json!({
        "auto_update": true,
        "telemetry": false,
        "current_profile": "kept",
        "profiles": {"kept": {"url": "http://127.0.0.1:1/api/v1/", "org": "tos"}}
    });
    std::fs::write(&config_path, original.to_string()).unwrap();
    let invoke = |args: &[&str]| {
        clean_cli()
            .env("HOME", home.path())
            .env("BRIEFCASE_HOME", &state)
            .env("BRIEFCASE_DAEMON_HOME", home.path().join("daemon"))
            .env("BRIEFCASE_AUTO_UPDATE", "on")
            .env("BRIEFCASE_TELEMETRY", "off")
            .args(args)
            .output()
            .unwrap()
    };
    let shown = invoke(&["config", "show", "--json"]);
    assert!(shown.status.success());
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["auto_update"], false);
    assert_eq!(shown["update_manager"], "honeycomb");
    for args in [
        vec!["config", "set", "auto-update", "on"],
        vec!["system", "update"],
    ] {
        let result = invoke(&args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Honeycomb"));
        let saved: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved, original);
    }
    for args in [
        vec!["config", "unset", "auto-update"],
        vec!["config", "set", "auto-update", "off"],
    ] {
        assert!(invoke(&args).status.success());
        let saved: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(saved["auto_update"], false);
        assert_eq!(saved["telemetry"], false);
        assert_eq!(saved["profiles"], original["profiles"]);
    }
    assert!(!home.path().join("daemon/installation").exists());
    assert!(!state.join("update.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn early_access_invalidation_refreshes_before_the_command_starts() {
    for unauthorized in [false, true] {
        let server = authenticated_server().await;
        let home = tempfile::tempdir().unwrap();
        write_state(
            home.path(),
            &server,
            &json!({
                "sessions": {"work": session("2099-01-01T00:00:00Z")},
                "production_credential_scopes": {"work": scope(&server, "tos")}
            }),
        );
        Mock::given(method("GET")).and(path("/api/v1/auth/status"))
            .and(header("authorization", "Bearer stored-access-must-not-leak"))
            .respond_with(if unauthorized {
                ResponseTemplate::new(401).set_body_json(json!({"error":{"code":"unauthenticated","message":"inactive"}}))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"authenticated":false,"actor":null,"organizations":[],"expires_at":null}))
            }).expect(1).mount(&server).await;
        Mock::given(method("POST")).and(path("/api/v1/auth/refresh"))
            .and(body_json(json!({"refresh_token":"stored-refresh-must-not-leak"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token":"recovered-access","refresh_token":"recovered-refresh","token_type":"Bearer",
                "expires_in":1800,"scope":"profile","org_id":"tos","organizations":["tos"],
                "actor":{"principal_id":ACTOR_ID,"type":"carbon","public_id":"c:tester"}
            }))).expect(1).mount(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/entries"))
            .and(header("authorization", "Bearer recovered-access"))
            .respond_with(ResponseTemplate::new(201).set_body_json(entry_document()))
            .expect(1)
            .mount(&server)
            .await;
        let output = briefcase(
            home.path(),
            &[
                "--no-verify".into(),
                "--json".into(),
                "mkdir".into(),
                "test-folder".into(),
                "--type".into(),
                "public".into(),
            ],
        )
        .await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.iter().map(|r| r.url.path()).collect::<Vec<_>>(),
            [
                "/api/v1/auth/status",
                "/api/v1/auth/refresh",
                "/api/v1/entries"
            ]
        );
        let credentials: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
                .unwrap();
        assert_eq!(
            credentials["sessions"]["work"]["refresh_token"],
            "recovered-refresh"
        );
        assert_eq!(
            credentials["production_credential_scopes"]["work"],
            scope(&server, "tos")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn early_invalidation_refresh_outage_keeps_the_original_family_and_receipt() {
    let server = authenticated_server().await;
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        &server,
        &json!({
            "sessions":{"work":session("2099-01-01T00:00:00Z")},
            "production_credential_scopes":{"work":scope(&server,"tos")}
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"authenticated":false,"actor":null,"organizations":[],"expires_at":null}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/refresh"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_json(json!({"error":{"code":"unavailable","message":"retry later"}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let output = briefcase(
        home.path(),
        &["--no-verify".into(), "--json".into(), "ls".into()],
    )
    .await;
    assert!(!output.status.success());
    let credentials: Value =
        serde_json::from_slice(&std::fs::read(home.path().join("credentials.json")).unwrap())
            .unwrap();
    let saved = &credentials["sessions"]["work"];
    assert_eq!(saved["refresh_token"], "stored-refresh-must-not-leak");
    assert_eq!(saved["access_token"], "stored-access-must-not-leak");
    assert!(saved["refresh_idempotency_key"].is_string());
    assert!(saved["refresh_started_at"].is_string());
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path() != "/api/v1/entries")
    );
}

const GRANT_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f830";

fn signed_in(server: &MockServer) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    write_state(
        home.path(),
        server,
        &json!({
            "sessions": {"work": session("2099-01-01T00:00:00Z")},
            "production_credential_scopes": {"work": scope(server, "tos")},
        }),
    );
    home
}

fn refusal(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({
        "error": {"code": code, "message": message, "request_id": null}
    }))
}

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture exercises every expiring command against the same grant"
)]
async fn expiring_shares_and_links_send_whole_minutes_and_say_when_they_end() {
    let server = authenticated_server().await;
    let home = signed_in(&server);
    let invitation = |expires_at: Value| {
        json!({
            "id": GRANT_ID,
            "principal": {"type": "email", "id": "alex@example.com"},
            "access": ["read"],
            "inherit": false,
            "expires_at": expires_at,
        })
    };
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/invitations")))
        .and(body_json(json!({
            "principal": {"type": "email", "id": "alex@example.com"},
            "access": ["read"],
            "inherit": false,
            "expires_in_minutes": 120,
        })))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(invitation(json!("2099-01-01T02:00:00Z"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/link-access")))
        .and(body_json(
            json!({"enabled": true, "expires_in_minutes": 10_080}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "can_manage": true, "enabled": true, "effective": true, "inherited_from": null,
            "url": "https://briefcase.example/org/tos/public/report.pdf",
            "expires_at": "2099-01-08T00:00:00Z",
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/api/v1/entries/{ENTRY_ID}/invitations/{GRANT_ID}"
        )))
        .and(body_json(json!({"expires_in_minutes": 4_320})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(invitation(json!("2099-01-04T00:00:00Z"))),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/api/v1/entries/{ENTRY_ID}/invitations/{GRANT_ID}"
        )))
        .and(body_json(json!({"permanent": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(invitation(Value::Null)))
        .expect(1)
        .mount(&server)
        .await;

    let output = briefcase(
        home.path(),
        &arguments(&[
            "--no-verify",
            "share",
            ENTRY_ID,
            "email:alex@example.com",
            "--expires-after",
            "2h",
        ]),
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Standard output stays the service's JSON; the reminder goes to stderr.
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["expires_at"], "2099-01-01T02:00:00Z");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "expiring share {GRANT_ID} for email:alex@example.com ends 2099-01-01 02:00 UTC (in "
        )),
        "{stderr}"
    );

    let output = briefcase(
        home.path(),
        &arguments(&["--no-verify", "link", ENTRY_ID, "--expires-after", "7d"]),
    )
    .await;
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["expires_at"], "2099-01-08T00:00:00Z");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expiring link ends 2099-01-08 00:00 UTC")
    );

    let output = briefcase(
        home.path(),
        &arguments(&[
            "--no-verify",
            "--json",
            "expiry",
            ENTRY_ID,
            GRANT_ID,
            "--expires-in",
            "3d",
        ]),
    )
    .await;
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["expires_at"], "2099-01-04T00:00:00Z");
    // JSON asked for, so nothing but the answer is printed anywhere.
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());

    let output = briefcase(
        home.path(),
        &arguments(&["--no-verify", "expiry", ENTRY_ID, GRANT_ID, "--permanent"]),
    )
    .await;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("now permanent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn expiring_refusals_explain_themselves_and_keep_their_exit_codes() {
    let server = authenticated_server().await;
    let home = signed_in(&server);

    // Refused locally: nothing reaches the server.
    let output = briefcase(
        home.path(),
        &arguments(&[
            "--no-verify",
            "share",
            ENTRY_ID,
            "c:cos",
            "--access",
            "read,write",
            "--expires-after",
            "1h",
        ]),
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("read-only") && stderr.contains("read,write"),
        "{stderr}"
    );
    let output = briefcase(
        home.path(),
        &arguments(&["--no-verify", "link", ENTRY_ID, "--expires-after", "31d"]),
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("43200 minutes"));
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );

    Mock::given(method("PATCH"))
        .and(path(format!(
            "/api/v1/entries/{ENTRY_ID}/invitations/{GRANT_ID}"
        )))
        .respond_with(refusal(404, "not_found", "Not found"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/link-access")))
        .respond_with(refusal(
            409,
            "link_already_permanent",
            "The link is already permanent",
        ))
        .mount(&server)
        .await;

    let output = briefcase(
        home.path(),
        &arguments(&[
            "--no-verify",
            "expiry",
            ENTRY_ID,
            GRANT_ID,
            "--expires-in",
            "1h",
        ]),
    )
    .await;
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("(not_found"), "{stderr}");
    assert!(stderr.contains("gone for good"), "{stderr}");

    let output = briefcase(
        home.path(),
        &arguments(&["--no-verify", "link", ENTRY_ID, "--expires-after", "1h"]),
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--enabled false"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture follows a self-destructing file from upload to deletion"
)]
async fn self_destructing_files_upload_keep_and_delete_for_good() {
    let server = authenticated_server().await;
    let home = signed_in(&server);
    let fresh = home.path().join("fresh.txt");
    let existing = home.path().join("existing.txt");
    std::fs::write(&fresh, b"gone soon").unwrap();
    std::fs::write(&existing, b"gone soon").unwrap();
    let mut self_destructing = entry_document();
    self_destructing["self_destruct_at"] = json!("2099-01-01T01:30:00Z");

    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains(
            "name=\"self_destruct_minutes\"\r\n\r\n90\r\n",
        ))
        .and(body_string_contains("fresh.txt"))
        .respond_with(ResponseTemplate::new(201).set_body_json(self_destructing.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/uploads"))
        .and(body_string_contains("existing.txt"))
        .respond_with(refusal(
            409,
            "self_destruct_requires_new_file",
            "Self destruct can only be set on a new file",
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(self_destructing))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/self-destruct")))
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/self-destruct")))
        .respond_with(refusal(403, "forbidden", "Not allowed"))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let output = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "put".into(),
            fresh.display().to_string(),
            DESTINATION_ID.into(),
            "--self-destruct".into(),
            "90m".into(),
        ],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(", self-destructs 2099-01-01 01:30 UTC (in ")
    );

    let output = briefcase(
        home.path(),
        &[
            "--no-verify".into(),
            "put".into(),
            existing.display().to_string(),
            DESTINATION_ID.into(),
            "--self-destruct".into(),
            "90".into(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("(self_destruct_requires_new_file") && stderr.contains("--name"),
        "{stderr}"
    );

    let output = briefcase(
        home.path(),
        &arguments(&["--no-verify", "--json", "keep", ENTRY_ID]),
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value,
        json!([{"entry_id": ENTRY_ID, "path": entry_document()["path"]}])
    );

    let output = briefcase(home.path(), &arguments(&["--no-verify", "keep", ENTRY_ID])).await;
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("only the creator"));

    let output = briefcase(home.path(), &arguments(&["--no-verify", "rm", ENTRY_ID])).await;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("deleted permanently"), "{stdout}");
    assert!(!stdout.contains("bin, recoverable"), "{stdout}");
}

#[test]
fn help_documents_the_expiring_and_self_destruct_rules() {
    let help = |args: &[&str]| {
        let output = clean_cli().env_remove("HOME").args(args).output().unwrap();
        assert!(output.status.success());
        // Help wraps to the terminal; compare words, not line breaks.
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let share = help(&["share", "--help"]);
    for rule in [
        "--expires-after <DURATION>",
        "read-only",
        "its own grant",
        "strict",
        "Nobody is notified",
        "30 days",
    ] {
        assert!(share.contains(rule), "share --help lacks {rule}");
    }
    let unshare = help(&["unshare", "--help"]);
    assert!(unshare.contains("expiring share early") && unshare.contains("no notification"));
    let expiring = help(&["expiry", "--help"]);
    assert!(expiring.contains("--expires-in <DURATION>") && expiring.contains("--permanent"));
    let link = help(&["link", "--help"]);
    assert!(link.contains("--expires-after <DURATION>") && link.contains("link_already_permanent"));
    let put = help(&["put", "--help"]);
    for rule in [
        "--self-destruct <DURATION>",
        "new file",
        "never",
        "briefcase keep",
    ] {
        assert!(put.contains(rule), "put --help lacks {rule}");
    }
    let keep = help(&["keep", "--help"]);
    assert!(keep.contains("creator") && keep.contains("admins and owners"));
    let find = help(&["find", "--help"]);
    assert!(find.contains("is:expiring") && find.contains("is:self-destruct"));
    let rm = help(&["rm", "--help"]);
    assert!(rm.contains("never enters the bin"));
}
