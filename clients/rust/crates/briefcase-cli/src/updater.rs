//! Default-on, best-effort maintenance of the installed `briefcase` binary.

use time::{Duration, OffsetDateTime};

use briefcase_client::update::{Release, Version, check, install_binary, install_binary_at};

use crate::{
    run::CliError,
    state::{StateDirectory, UpdateState},
};

/// Published CLI crate.
pub const CLI_CRATE: &str = "briefcase-cli";
/// Binary installed by the CLI crate.
pub const CLI_BINARY: &str = "briefcase";
/// Version executing this invocation.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::hours(1);

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

/// Runs maintenance for a registered Silicon home without changing process environment.
pub async fn automatic_for_state(state: &StateDirectory) -> Result<Outcome, CliError> {
    let configuration = state.configuration()?;
    if !environment_switch().unwrap_or(configuration.auto_update) {
        return Ok(Outcome::Skipped);
    }
    let installation = StateDirectory::at(crate::daemon::root()?.join("installation"));
    let Some(_lock) = installation.try_lock_update()? else {
        return Ok(Outcome::Skipped);
    };
    let update_state = installation.update_state()?;
    if !check_is_due(&update_state, OffsetDateTime::now_utc()) {
        return Ok(Outcome::Skipped);
    }
    update_locked(&installation, &update_state).await
}

/// Checks immediately, irrespective of policy or throttle state.
///
/// # Errors
///
/// Returns an error when crates.io, Cargo, or local state cannot be used.
pub async fn update_now() -> Result<Outcome, CliError> {
    let state = StateDirectory::at(crate::daemon::root()?.join("installation"));
    let _lock = state.try_lock_update()?.ok_or_else(|| {
        CliError::Usage(
            "another Briefcase updater is already running; retry after it finishes".to_owned(),
        )
    })?;
    let update_state = state.update_state()?;
    update_locked(&state, &update_state).await
}

async fn update_locked(
    state: &StateDirectory,
    previous: &UpdateState,
) -> Result<Outcome, CliError> {
    let known_version = known_installed_version(previous);
    // Persist before network/Cargo, while holding the independent update lock.
    // A failed or interrupted attempt is throttled too, and an old process
    // cannot overwrite another invocation's record of a newer installation.
    state.save_update_state(&UpdateState {
        checked_version: Some(known_version.clone()),
        checked_at: Some(OffsetDateTime::now_utc()),
    })?;
    let release = check(CLI_CRATE, &known_version).await?;
    let outcome = tokio::task::spawn_blocking(move || apply_release(&release))
        .await
        .map_err(|error| CliError::Usage(format!("updater task failed: {error}")))??;
    let checked_version = match &outcome {
        Outcome::Updated { to, .. } | Outcome::Current(to) => to.to_string(),
        Outcome::Skipped => CLI_VERSION.to_owned(),
    };
    state.save_update_state(&UpdateState {
        checked_version: Some(checked_version),
        checked_at: Some(OffsetDateTime::now_utc()),
    })?;
    Ok(outcome)
}

fn apply_release(release: &Release) -> Result<Outcome, CliError> {
    if !release.update_available() {
        return Ok(Outcome::Current(release.current.clone()));
    }
    let executable = std::env::current_exe().map_err(|source| CliError::Io {
        path: "CLI executable".into(),
        source,
    })?;
    if let Some(bin) = executable
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "bin"))
        && let Some(root) = bin.parent()
    {
        install_binary_at(CLI_CRATE, CLI_BINARY, &release.latest, root)?;
    } else {
        install_binary(CLI_CRATE, CLI_BINARY, &release.latest)?;
    }
    Ok(Outcome::Updated {
        from: release.current.clone(),
        to: release.latest.clone(),
    })
}

fn check_is_due(state: &UpdateState, now: OffsetDateTime) -> bool {
    state.checked_at.is_none_or(|checked_at| {
        // Recover from wall-clock corrections without indefinitely suppressing
        // checks. A newer binary does not bypass another invocation's throttle.
        now < checked_at || now - checked_at >= CHECK_INTERVAL
    })
}

fn known_installed_version(state: &UpdateState) -> String {
    match (
        state
            .checked_version
            .as_deref()
            .and_then(|value| Version::parse(value).ok()),
        Version::parse(CLI_VERSION).ok(),
    ) {
        (Some(saved), Some(running)) if saved > running => saved.to_string(),
        _ => CLI_VERSION.to_owned(),
    }
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
    use time::OffsetDateTime;

    use super::{CHECK_INTERVAL, CLI_VERSION, check_is_due, known_installed_version};
    use crate::state::UpdateState;

    #[test]
    fn checks_at_most_hourly_after_an_attempt() {
        let now = OffsetDateTime::now_utc();
        assert!(check_is_due(&UpdateState::default(), now));
        let fresh = UpdateState {
            checked_version: Some(CLI_VERSION.to_owned()),
            checked_at: Some(now),
        };
        assert!(!check_is_due(&fresh, now));
        assert!(!check_is_due(
            &fresh,
            now + CHECK_INTERVAL - time::Duration::seconds(1)
        ));
        assert!(check_is_due(&fresh, now + CHECK_INTERVAL));
    }

    #[test]
    fn backward_clock_corrections_do_not_disable_maintenance() {
        let now = OffsetDateTime::now_utc();
        let state = UpdateState {
            checked_version: Some(CLI_VERSION.to_owned()),
            checked_at: Some(now + CHECK_INTERVAL),
        };
        assert!(check_is_due(&state, now));
    }

    #[test]
    fn old_processes_preserve_a_newer_install_record() {
        let state = UpdateState {
            checked_version: Some("9999.0.0".to_owned()),
            checked_at: None,
        };
        assert_eq!(known_installed_version(&state), "9999.0.0");
        assert_eq!(
            known_installed_version(&UpdateState::default()),
            CLI_VERSION
        );
    }

    #[test]
    fn a_new_binary_version_respects_the_shared_hourly_throttle() {
        let state = UpdateState {
            checked_version: Some("0.0.0".to_owned()),
            checked_at: Some(OffsetDateTime::now_utc()),
        };
        assert!(!check_is_due(&state, OffsetDateTime::now_utc()));
    }
}
