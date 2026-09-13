//! Shared-service process checks. No system service or external registry is used.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Value;
use std::os::unix::fs::PermissionsExt as _;
use std::{
    path::Path,
    process::{Command, Output},
    time::Duration,
};

fn cli(runtime: &Path, state: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .env("BRIEFCASE_TELEMETRY", "off")
        .env("BRIEFCASE_DAEMON_HOME", runtime)
        .env("BRIEFCASE_HOME", state)
        .env("BRIEFCASE_AUTO_UPDATE", "off")
        .env_remove("BRIEFCASE_TEST")
        .env_remove("BRIEFCASE_APP_SECRET")
        .args(args)
        .output()
        .unwrap()
}

struct Cleanup {
    runtime: std::path::PathBuf,
    state: std::path::PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = cli(&self.runtime, &self.state, &["daemon", "stop"]);
    }
}

#[test]
fn two_silicon_homes_share_one_live_daemon_and_can_stop_it() {
    // macOS Unix sockets have a short pathname limit; use a short temp root.
    let dir = tempfile::Builder::new()
        .prefix("bc-")
        .tempdir_in("/tmp")
        .unwrap();
    let runtime = dir.path().join("r");
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    let _cleanup = Cleanup {
        runtime: runtime.clone(),
        state: first.clone(),
    };
    let start = cli(&runtime, &first, &["daemon", "start"]);
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stderr)
    );
    let before: Value =
        serde_json::from_slice(&cli(&runtime, &first, &["daemon", "status"]).stdout).unwrap();
    assert_eq!(before["running"], true);
    let second_start = cli(&runtime, &second, &["daemon", "start"]);
    assert!(second_start.status.success());
    let after: Value =
        serde_json::from_slice(&cli(&runtime, &second, &["daemon", "status"]).stdout).unwrap();
    assert_eq!(before["pid"], after["pid"]);
    assert_eq!(after["homes"].as_array().unwrap().len(), 2);
    assert_eq!(
        std::fs::metadata(runtime.join("control.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(!first.join("update.json").exists());
    assert!(!second.join("update.json").exists());
    assert!(cli(&runtime, &first, &["daemon", "stop"]).status.success());
    for _ in 0..30 {
        if !runtime.join("control.sock").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let stopped: Value =
        serde_json::from_slice(&cli(&runtime, &first, &["daemon", "status"]).stdout).unwrap();
    assert_eq!(stopped["running"], false);
    // A fresh instance reloads both registrations, without credentials in IPC.
    assert!(cli(&runtime, &first, &["daemon", "start"]).status.success());
    let restarted: Value =
        serde_json::from_slice(&cli(&runtime, &first, &["daemon", "status"]).stdout).unwrap();
    assert_eq!(restarted["homes"].as_array().unwrap().len(), 2);
}

#[test]
fn test_selection_footer_survives_argument_errors_without_echoing_a_secret() {
    let dir = tempfile::tempdir().unwrap();
    let result = cli(
        dir.path(),
        dir.path(),
        &["--app-secret", "never-print-this-secret", "not-a-command"],
    );
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("TEST ENVIRONMENT"));
    assert!(!stderr.contains("never-print-this-secret"));
    assert!(result.stdout.is_empty());
}

#[test]
fn manuals_are_bundled_and_readable_without_a_login_or_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let result = cli(dir.path(), dir.path(), &["docs", "testing", "--json"]);
    assert!(result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(value["markdown"].as_str().unwrap().contains("2 GiB"));
    assert!(!dir.path().join("control.sock").exists());
}

#[test]
#[cfg(target_os = "macos")]
fn installing_a_login_service_writes_valid_private_configuration() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::Builder::new()
        .prefix("bc-")
        .tempdir_in("/tmp")
        .unwrap();
    let runtime = dir.path().join("r");
    let state = dir.path().join("state");
    let home = dir.path().join("home & space");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let control = bin.join("launchctl");
    std::fs::write(&control, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _cleanup = Cleanup {
        runtime: runtime.clone(),
        state: state.clone(),
    };
    let result = Command::new(env!("CARGO_BIN_EXE_briefcase"))
        .env("BRIEFCASE_TELEMETRY", "off")
        .env("HOME", &home)
        .env("BRIEFCASE_HOME", &state)
        .env("BRIEFCASE_DAEMON_HOME", &runtime)
        .env("BRIEFCASE_AUTO_UPDATE", "off")
        .env("BRIEFCASE_TOKEN", "not-for-service-definition")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .args(["daemon", "install"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let plist = home.join("Library/LaunchAgents/com.teamofsilicons.briefcase.plist");
    assert!(
        Command::new("/usr/bin/plutil")
            .args(["-lint"])
            .arg(&plist)
            .status()
            .unwrap()
            .success()
    );
    let text = std::fs::read_to_string(plist).unwrap();
    assert!(text.contains("home &amp; space"));
    assert!(!text.contains("not-for-service-definition"));
}

#[test]
fn putting_a_secret_in_the_uuid_flag_does_not_leak_it_in_parser_errors() {
    let dir = tempfile::tempdir().unwrap();
    let secret = format!("ask_{}", "a".repeat(43));
    let result = cli(dir.path(), dir.path(), &["--test", &secret, "ls"]);
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!stderr.contains(&secret));
    assert!(stderr.contains("<redacted>"));
    assert!(stderr.contains("TEST ENVIRONMENT"));
}
