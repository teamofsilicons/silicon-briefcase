//! The retired background service and the offline guides, process by process.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{Home, briefcase, stderr, stdout};
use serde_json::Value;

#[tokio::test]
async fn uninstalling_with_nothing_installed_succeeds_and_changes_nothing() {
    let home = Home::new();
    let output = briefcase(
        &home,
        &["daemon", "uninstall"],
        &[("BRIEFCASE_DAEMON_HOME", "/nonexistent-briefcase-daemon")],
    )
    .await;
    assert!(output.status.success(), "{}", stderr(&output));
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["installed"], false);
    assert_eq!(answer["service_removed"], Value::Null);
    assert_eq!(answer["stopped"], false);
    assert!(!home.state().exists());
}

#[tokio::test]
async fn the_guides_are_bundled_and_readable_without_a_sign_in() {
    let home = Home::new();
    let listing = briefcase(&home, &["docs"], &[]).await;
    assert!(listing.status.success());
    let listing: Value = serde_json::from_slice(&listing.stdout).unwrap();
    assert_eq!(listing["guides"], serde_json::json!(["cli", "client"]));
    for guide in ["cli", "client"] {
        let output = briefcase(&home, &["docs", guide], &[]).await;
        assert!(output.status.success(), "{guide}");
        let text = stdout(&output);
        assert!(text.len() > 1000, "{guide} is too short");
        for retired in ["Honeycomb", "honeycomb", "IAM", "--org", "organization"] {
            assert!(!text.contains(retired), "{guide} mentions {retired}");
        }
    }
    let unknown = briefcase(&home, &["docs", "testing"], &[]).await;
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("available: cli, client"));
}

#[tokio::test]
async fn every_command_explains_itself_in_its_help() {
    let home = Home::new();
    for command in [
        "login", "logout", "accounts", "status", "drives", "shared", "allow", "ls", "find",
        "search", "stat", "mkdir", "put", "get", "cat", "archive", "mv", "rm", "keep", "bin",
        "versions", "restore", "history", "logs", "link", "share", "unshare", "expiry", "shares",
        "access", "inbox", "usage", "storage", "app", "config", "report", "daemon", "docs",
        "version",
    ] {
        let output = briefcase(&home, &[command, "--help"], &[]).await;
        assert!(output.status.success(), "{command}");
        let text = stdout(&output);
        assert!(text.contains("Usage: briefcase"), "{command}");
        assert!(
            text.contains("briefcase docs cli"),
            "{command} has no pointer to the guide"
        );
        for retired in ["--org", "IAM", "Honeycomb", "organization", "tag:"] {
            assert!(
                !text.contains(retired),
                "{command} --help mentions {retired}"
            );
        }
    }
}
