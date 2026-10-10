//! Shared harness for the CLI's process-level tests: the binary runs in a
//! cleared environment against mock Briefcase and Silicon Accounts servers.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

pub const UUID: &str = "zQo";
pub const ID: &str = "c:ada";
pub const ENTRY_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f828";
pub const FOLDER_ID: &str = "01a067ce-7f19-7790-820a-0be6b3d4f829";

/// A test's own state: a home for the CLI (`BRIEFCASE_HOME`) and a separate
/// `HOME`, both temporary.
pub struct Home {
    root: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
        }
    }

    /// The CLI's state directory.
    pub fn state(&self) -> PathBuf {
        self.root.path().join("state")
    }

    /// The `HOME` the CLI sees.
    pub fn home(&self) -> PathBuf {
        let home = self.root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    pub fn credentials(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.state().join("credentials.json")).unwrap())
            .unwrap()
    }

    pub fn configuration(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.state().join("config.json")).unwrap()).unwrap()
    }

    /// Saves a signed-in profile `default` for `api` and `accounts`, whose
    /// access token expires `expires_in_seconds` from now.
    pub fn sign_in(&self, api: &str, accounts: &str, expires_in_seconds: i64) {
        let state = self.state();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("config.json"),
            serde_json::to_vec_pretty(&json!({
                "telemetry": false,
                "current_profile": "default",
                "profiles": {"default": {"url": api, "accounts_url": accounts}}
            }))
            .unwrap(),
        )
        .unwrap();
        let expires_at =
            time::OffsetDateTime::now_utc() + time::Duration::seconds(expires_in_seconds);
        std::fs::write(
            state.join("credentials.json"),
            serde_json::to_vec_pretty(&json!({
                "format": 2,
                "sessions": {"default": {
                    "api_url": api,
                    "accounts_url": accounts,
                    "app_id": "briefcase",
                    "account": {"uuid": UUID, "id": ID, "kind": "carbon", "display_name": "Ada"},
                    "access_token": "access-one",
                    "refresh_token": "sar_one",
                    "expires_at": expires_at.format(&time::format_description::well_known::Rfc3339).unwrap(),
                    "refresh_expires_at": null,
                    "scope": ["profile"],
                    "signed_in_at": "2026-10-10T10:00:00Z",
                    "method": "device"
                }},
                "pending_mutations": {}
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

/// Runs the CLI with a cleared environment: only `PATH`, the test's homes,
/// telemetry off, and `extra`.
pub async fn briefcase(home: &Home, arguments: &[&str], extra: &[(&str, &str)]) -> Output {
    run(home, arguments, extra, None).await
}

/// Like [`briefcase`], writing `input` to standard input.
pub async fn briefcase_with_stdin(
    home: &Home,
    arguments: &[&str],
    extra: &[(&str, &str)],
    input: &[u8],
) -> Output {
    run(home, arguments, extra, Some(input.to_owned())).await
}

async fn run(
    home: &Home,
    arguments: &[&str],
    extra: &[(&str, &str)],
    input: Option<Vec<u8>>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_briefcase"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.home())
        .env("BRIEFCASE_HOME", home.state())
        .env("BRIEFCASE_TELEMETRY", "off")
        .env("HOSTNAME", "testhost")
        .args(arguments);
    for (name, value) in extra {
        command.env(name, value);
    }
    tokio::task::spawn_blocking(move || {
        if let Some(input) = input {
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("the test CLI must start");
            child
                .stdin
                .take()
                .expect("piped stdin")
                .write_all(&input)
                .expect("stdin must be written");
            child.wait_with_output().expect("the test CLI must finish")
        } else {
            command.output().expect("the test CLI must run")
        }
    })
    .await
    .expect("the CLI process must join")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The JSON error a `--json` run printed on standard error.
pub fn json_error(output: &Output) -> Value {
    let text = stderr(output);
    let line = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON error on stderr: {text}"));
    serde_json::from_str(line).unwrap()
}

pub fn version_document() -> Value {
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
        "contract_version": "4.0.0",
        "build": "4.0.0",
        "operations": operations,
    })
}

/// A mock Briefcase that serves its contract and says it trusts `accounts`.
pub async fn briefcase_service(accounts: &str) -> MockServer {
    let server = MockServer::start().await;
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
        .and(path("/api/v1/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "app_id": "briefcase",
            "accounts_url": accounts,
            "delegated_scopes": []
        })))
        .mount(&server)
        .await;
    server
}

pub fn api(server: &MockServer) -> String {
    format!("{}/api/v1/", server.uri())
}

/// A token response for the account `zQo` / `c:ada`.
pub fn tokens(access: &str, refresh: &str) -> Value {
    json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": 1800,
        "refresh_token": refresh,
        "refresh_token_expires_at": "2028-01-01T00:00:00.000Z",
        "scope": "profile email",
        "membership_id": "briefcase:zQo",
        "account": {
            "uuid": UUID, "membership_id": "briefcase:zQo", "kind": "carbon",
            "id": ID, "display_name": "Ada", "pfp_url": ""
        }
    })
}

pub fn oauth_error(status: u16, error: &str, description: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_json(json!({"error": error, "error_description": description}))
}

pub fn entry(id: &str, drive: &str, name: &str, path: &str, folder: bool) -> Value {
    json!({
        "id": id,
        "org_id": drive,
        "type": if folder { "folder" } else { "file" },
        "visibility": "full",
        "name": name,
        "path": path,
        "parent_id": if folder { Value::Null } else { json!(FOLDER_ID) },
        "root_type": "private",
        "tag": null,
        "content_type": if folder { Value::Null } else { json!("text/plain") },
        "size": if folder { Value::Null } else { json!(4) },
        "render": if folder { Value::Null } else { json!("code") },
        "permanent_url": format!("https://briefcase.example/{ID}/{path}"),
        "content_url": null,
        "download_url": null,
        "owner": {"type": "carbon", "id": drive},
        "origin_app_id": null,
        "effective_access": ["read", "write", "update", "delete", "manage_permissions"],
        "created_at": "2026-10-10T10:00:00Z",
        "updated_at": "2026-10-10T10:00:00Z",
        "deleted_at": null
    })
}

/// Whether `path`'s permissions are exactly `mode`.
#[cfg(unix)]
pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
