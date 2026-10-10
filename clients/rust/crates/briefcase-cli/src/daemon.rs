//! Removing the background service older releases installed.
//!
//! briefcase 3 could run a per-user daemon under launchd (macOS) or systemd
//! (Linux). It did no file work and existed for update bookkeeping, which
//! Silicon Apps now does. `briefcase daemon uninstall` stops a running one and
//! removes its login service; `briefcase daemon run` — what an old login
//! service still starts — removes that service and exits, so it is not
//! restarted forever.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::json;

use crate::{
    cli::DaemonCommand,
    error::{CliError, Result},
    render::Output,
};

const LABEL: &str = "com.teamofsilicons.briefcase";
const SYSTEMD_UNIT: &str = "briefcase.service";

/// What the retired subcommands should use instead.
const REPLACEMENT: &str = "briefcase no longer runs a background service: Silicon Apps keeps the CLI \
    up to date. Remove an old one with `briefcase daemon uninstall`.";

/// Runs a `daemon` subcommand.
///
/// # Errors
///
/// Returns `retired` for start/status/stop/install, and an I/O error when an
/// existing service definition cannot be removed.
#[allow(
    clippy::unused_async,
    reason = "dispatched with the other commands, which all run on the async runtime"
)]
pub async fn command(command: &DaemonCommand, output: Output) -> Result<()> {
    match command {
        DaemonCommand::Uninstall => {
            let stopped = stop_running_daemon();
            let removed = remove_login_service(home().as_deref(), false)?;
            output.json(&json!({
                "installed": false,
                "service_removed": removed.map(|path| path.display().to_string()),
                "stopped": stopped,
            }));
            Ok(())
        }
        DaemonCommand::Run => {
            eprintln!(
                "briefcase: the background service is retired; removing the login service that started it. {REPLACEMENT}"
            );
            // Best effort: whatever happens, exit 0 so the supervisor stops caring.
            let _ = remove_login_service(home().as_deref(), true);
            Ok(())
        }
        DaemonCommand::Start
        | DaemonCommand::Status
        | DaemonCommand::Stop
        | DaemonCommand::Install => Err(CliError::retired("`briefcase daemon`", REPLACEMENT)),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Where an older release put the login service definition.
fn service_file(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join(format!("Library/LaunchAgents/{LABEL}.plist"))
    } else {
        home.join(format!(".config/systemd/user/{SYSTEMD_UNIT}"))
    }
}

/// Removes the login service an older release installed, if any, returning
/// the file that was removed. From inside the service (`daemon run`) the file
/// goes first and the supervisor is told last, because telling it ends this
/// process.
fn remove_login_service(home: Option<&Path>, from_inside: bool) -> Result<Option<PathBuf>> {
    let Some(home) = home else {
        return Ok(None);
    };
    let file = service_file(home);
    if !file.exists() {
        return Ok(None);
    }
    if cfg!(target_os = "macos") {
        std::fs::remove_file(&file).map_err(|source| io(&file, source))?;
        if let Some(uid) = user_id() {
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/{LABEL}")])
                .output();
        }
    } else if cfg!(target_os = "linux") {
        // `--now` would stop this very process when it runs inside the unit.
        let disable: &[&str] = if from_inside {
            &["--user", "disable", SYSTEMD_UNIT]
        } else {
            &["--user", "disable", "--now", SYSTEMD_UNIT]
        };
        let _ = Command::new("systemctl").args(disable).output();
        std::fs::remove_file(&file).map_err(|source| io(&file, source))?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
    } else {
        std::fs::remove_file(&file).map_err(|source| io(&file, source))?;
    }
    Ok(Some(file))
}

fn user_id() -> Option<String> {
    let output = Command::new("id").arg("-u").output().ok()?;
    let uid = String::from_utf8(output.stdout).ok()?;
    let uid = uid.trim();
    (!uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit())).then(|| uid.to_owned())
}

/// Asks a running old daemon to stop over its private control socket.
#[cfg(unix)]
fn stop_running_daemon() -> bool {
    use std::io::{BufRead as _, BufReader, Write as _};
    let runtime = std::env::var_os("BRIEFCASE_DAEMON_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".briefcase-daemon")));
    let Some(runtime) = runtime else {
        return false;
    };
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(runtime.join("control.sock"))
    else {
        return false;
    };
    let timeout = Some(std::time::Duration::from_secs(3));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    if stream.write_all(b"{\"command\":\"stop\"}\n").is_err() {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.contains("\"running\":false")
}

#[cfg(not(unix))]
fn stop_running_daemon() -> bool {
    false
}

fn io(path: &Path, source: std::io::Error) -> CliError {
    CliError::Io {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::{remove_login_service, service_file};

    #[test]
    fn nothing_installed_means_nothing_to_remove() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            remove_login_service(Some(home.path()), false).unwrap(),
            None
        );
        assert_eq!(remove_login_service(None, false).unwrap(), None);
    }

    #[test]
    fn the_service_file_lives_where_older_releases_put_it() {
        let file = service_file(std::path::Path::new("/h"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                file,
                std::path::Path::new("/h/Library/LaunchAgents/com.teamofsilicons.briefcase.plist")
            );
        } else {
            assert_eq!(
                file,
                std::path::Path::new("/h/.config/systemd/user/briefcase.service")
            );
        }
    }
}
