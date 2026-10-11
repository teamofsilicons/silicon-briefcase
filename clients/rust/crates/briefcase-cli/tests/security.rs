//! Process-level checks that credentials never cross a saved trust boundary.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{path::Path, process::Command};

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
            "public_id": "c:tester",
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
        "path": "apps/notes/private/c:tester/note.txt",
        "parent_id": null,
        "root_type": "private",
        "tag": null,
        "content_type": "text/plain",
        "size": 4,
        "render": "document",
        "permanent_url": "https://briefcase.example/org/tos/apps/notes/private/c:tester/note.txt",
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": "c:tester"},
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
    let mut credentials = credentials.clone();
    credentials["format"] = json!(2);
    if let Some(sessions) = credentials["sessions"].as_object_mut() {
        for value in sessions.values_mut() {
            value["api_url"] = json!(format!("{}/api/v1/", server.uri()));
            value["accounts_url"] = json!(server.uri());
            value["app_id"] = json!("briefcase");
            value["account"] = json!({"uuid": ACTOR_ID, "id": "c:tester", "kind": "carbon"});
            value["signed_in_at"] = json!("2026-10-10T00:00:00Z");
            value["method"] = json!("device");
            value["scope"] = json!([]);
        }
    }
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(
        home.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "auto_update": false,
            "current_profile": "work",
            "profiles": {
                "work": {
                    "url": format!("{}/api/v1/", server.uri()),
                            }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        home.join("credentials.json"),
        serde_json::to_vec_pretty(&credentials).unwrap(),
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
    second_entry["path"] = json!("apps/notes/private/c:tester/second.txt");
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
    }));
    let move_fingerprint = fingerprint(&json!({
        "operation": "update-entry",
        "profile": "work",
        "url": api_url,
        "target": source,
        "destination": move_destination,
    }));
    let mkdir_address = fingerprint(&json!({
        "drive": null,
        "path": mkdir_path,
    }));
    let mkdir_fingerprint = fingerprint(&json!({
        "operation": "create-folder",
        "profile": "work",
        "url": api_url,
        "name": "new-folder",
        "drive": null,
        "parent": parent,
        "invitees": [],
    }));
    let upload_destination = parent.to_owned();
    let upload_address = fingerprint(&json!({
        "source": canonical_upload,
        "destination": upload_destination,
        "file_name": "upload.txt",
    }));
    let upload_fingerprint = fingerprint(&json!({
        "operation": "upload-file",
        "profile": "work",
        "url": api_url,
        "source": canonical_upload,
        "destination": upload_destination,
        "file_name": "upload.txt",
        "content_type": briefcase_client::guess_content_type("upload.txt"),
        "content_sha256": format!("{:x}", Sha256::digest(b"stable upload")),
    }));

    let move_scope = format!("entry:move:work:{move_address}");
    let mkdir_scope = format!("entry:mkdir:work:{mkdir_address}");
    let upload_scope = format!("entry:put:work:{upload_address}");
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
            "principal": {"type": "carbon", "id": "c:alex"},
            "access": ["read"],
            "inherit": false,
            "expires_at": expires_at,
        })
    };
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/entries/{ENTRY_ID}/invitations")))
        .and(body_json(json!({
            "principal": {"type": "carbon", "id": "c:alex"},
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
            "c:alex",
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
            "expiring share {GRANT_ID} for c:alex ends 2099-01-01 02:00 UTC (in "
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
