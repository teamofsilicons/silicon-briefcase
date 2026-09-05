//! Default-on, best-effort maintenance of the installed `briefcase` binary.

use time::{Duration, OffsetDateTime};

use briefcase_client::update::{Release, Version, check, install_binary};

use crate::{
    cli::{Command, ConfigCommand, SystemCommand},
    run::CliError,
    state::{StateDirectory, UpdateState},
};

/// Published CLI crate.
pub const CLI_CRATE: &str = "briefcase-cli";
/// Binary installed by the CLI crate.
pub const CLI_BINARY: &str = "briefcase";
/// Version executing this invocation.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::days(1);

/// Result of an automatic or explicit CLI update check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The check was disabled, not due, or controlled by this command.
    Skipped,
    /// The installed version is current.
    Current(Version),
    /// Cargo installed a newer binary for the next invocation.
    Updated { from: Version, to: Version },
}

/// Runs one due check before an ordinary command.
///
/// # Errors
///
/// Returns non-fatally to `main` when config, crates.io, or Cargo is unavailable.
pub async fn automatic(command: &Command) -> Result<Outcome, CliError> {
    if defers_automatic_update(command) {
        return Ok(Outcome::Skipped);
    }
    let state = StateDirectory::locate()?;
    let configuration = state.configuration()?;
    if !environment_switch().unwrap_or(configuration.auto_update) {
        return Ok(Outcome::Skipped);
    }
    let update_state = state.update_state()?;
    if !check_is_due(&update_state, OffsetDateTime::now_utc()) {
        return Ok(Outcome::Skipped);
    }
    let outcome = update_now().await;
    if outcome.is_err() {
        // A broken network or registry should produce at most one warning per
        // day, not delay every CLI command until connectivity returns.
        let _ = state.save_update_state(&UpdateState {
            checked_version: Some(CLI_VERSION.to_owned()),
            checked_at: Some(OffsetDateTime::now_utc()),
        });
    }
    outcome
}

/// Checks immediately, irrespective of policy or throttle state.
///
/// # Errors
///
/// Returns an error when crates.io, Cargo, or local state cannot be used.
pub async fn update_now() -> Result<Outcome, CliError> {
    let release = check(CLI_CRATE, CLI_VERSION).await?;
    let outcome = apply_release(&release)?;
    let checked_version = match &outcome {
        Outcome::Updated { to, .. } | Outcome::Current(to) => to.to_string(),
        Outcome::Skipped => CLI_VERSION.to_owned(),
    };
    StateDirectory::locate()?.save_update_state(&UpdateState {
        checked_version: Some(checked_version),
        checked_at: Some(OffsetDateTime::now_utc()),
    })?;
    Ok(outcome)
}

fn apply_release(release: &Release) -> Result<Outcome, CliError> {
    if !release.update_available() {
        return Ok(Outcome::Current(release.current.clone()));
    }
    install_binary(CLI_CRATE, CLI_BINARY, &release.latest)?;
    Ok(Outcome::Updated {
        from: release.current.clone(),
        to: release.latest.clone(),
    })
}

fn defers_automatic_update(command: &Command) -> bool {
    // Login SLTs and application OBO proofs are short-lived, one-use
    // credentials. Never spend their lifetime on registry or Cargo work; the
    // next ordinary invocation performs the due check instead.
    matches!(command, Command::Login(_) | Command::App(_))
        || matches!(command, Command::System(SystemCommand::Update))
        || matches!(
            command,
            Command::Config(ConfigCommand::Set { key, .. } | ConfigCommand::Unset { key })
                if key == "auto-update"
        )
}

fn check_is_due(state: &UpdateState, now: OffsetDateTime) -> bool {
    state.checked_version.as_deref() != Some(CLI_VERSION)
        || state
            .checked_at
            .is_none_or(|checked_at| now - checked_at >= CHECK_INTERVAL)
}

fn environment_switch() -> Option<bool> {
    match std::env::var("BRIEFCASE_AUTO_UPDATE")
        .ok()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use time::OffsetDateTime;

    use super::{CHECK_INTERVAL, CLI_VERSION, check_is_due, defers_automatic_update};
    use crate::{cli::Cli, state::UpdateState};

    #[test]
    fn each_binary_version_checks_at_most_daily_after_success() {
        let now = OffsetDateTime::now_utc();
        assert!(check_is_due(&UpdateState::default(), now));
        let fresh = UpdateState {
            checked_version: Some(CLI_VERSION.to_owned()),
            checked_at: Some(now),
        };
        assert!(!check_is_due(&fresh, now));
        assert!(check_is_due(&fresh, now + CHECK_INTERVAL));
    }

    #[test]
    fn a_new_binary_version_checks_immediately() {
        let state = UpdateState {
            checked_version: Some("0.0.0".to_owned()),
            checked_at: Some(OffsetDateTime::now_utc()),
        };
        assert!(check_is_due(&state, OffsetDateTime::now_utc()));
    }

    #[test]
    fn short_lived_credentials_and_updater_controls_skip_the_pre_command_check() {
        let login = Cli::try_parse_from(["briefcase", "login", "--slt", "slt-once"]).unwrap();
        assert!(defers_automatic_update(&login.command));

        let app = Cli::try_parse_from([
            "briefcase",
            "app",
            "upload",
            "--app-id",
            "tos>notes",
            "--proof",
            "obo-once",
            "note.txt",
        ])
        .unwrap();
        assert!(defers_automatic_update(&app.command));

        let explicit = Cli::try_parse_from(["briefcase", "system", "update"]).unwrap();
        assert!(defers_automatic_update(&explicit.command));
        let policy =
            Cli::try_parse_from(["briefcase", "config", "set", "auto-update", "off"]).unwrap();
        assert!(defers_automatic_update(&policy.command));

        let ordinary = Cli::try_parse_from(["briefcase", "ls"]).unwrap();
        assert!(!defers_automatic_update(&ordinary.command));
    }
}
