//! Operational telemetry for CLI commands: an opt-out preference here, delivery
//! by the package (through Briefcase's own `/api/v1/telemetry`, so table keys
//! stay on the server). Events carry a command name, a stage and a duration —
//! never arguments, paths, ids or tokens.

use std::time::Instant;

use briefcase_client::telemetry::{Event, Source, Stage};

use crate::{
    cli::{Cli, Command, LoginCommand},
    state::StateDirectory,
};

/// Whether telemetry is on: `BRIEFCASE_TELEMETRY` and `briefcase config set telemetry`.
pub fn enabled() -> bool {
    briefcase_client::telemetry::enabled_from_env()
        && StateDirectory::locate()
            .and_then(|state| state.configuration())
            .is_ok_and(|config| config.telemetry)
}

/// One command's start, finished with its outcome.
pub struct CommandObservation {
    started: Instant,
    event: Event,
    url: String,
    enabled: bool,
}

impl CommandObservation {
    /// Starts observing a command. Discovery and local commands are never
    /// reported: they must work offline and send nothing.
    pub fn new(cli: &Cli) -> Self {
        let event = Event::new(Source::Cli, name(&cli.command), Stage::Started);
        let url = crate::session::select(&cli.global).map_or_else(
            |_| briefcase_client::DEFAULT_API_URL.to_owned(),
            |selection| selection.api_url,
        );
        Self {
            started: Instant::now(),
            event,
            url,
            enabled: reported(&cli.command) && enabled(),
        }
    }

    /// Sends the outcome, best effort.
    pub async fn finish(mut self, success: bool) {
        if !self.enabled {
            return;
        }
        self.event.stage = if success {
            Stage::Completed
        } else {
            Stage::Failed
        };
        self.event.duration_ms =
            Some(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX));
        let _ = briefcase_client::telemetry::submit(&self.url, &self.event).await;
    }
}

fn reported(command: &Command) -> bool {
    !matches!(
        command,
        Command::Accounts
            | Command::Iam
            | Command::Login(_)
            | Command::Logout
            | Command::Docs { .. }
            | Command::Version
            | Command::Config(_)
            | Command::Daemon(_)
            | Command::Env { .. }
            | Command::System { .. }
    )
}

fn name(command: &Command) -> &'static str {
    match command {
        Command::Login(args) => match args.command {
            Some(LoginCommand::Status { .. }) => "login_status",
            None => "login",
        },
        Command::Logout => "logout",
        Command::Accounts | Command::Iam => "accounts",
        Command::Status => "status",
        Command::Drives => "drives",
        Command::Shared(_) => "shared",
        Command::Allow(_) => "allow",
        Command::Ls(_) => "ls",
        Command::Find(_) => "find",
        Command::Search(_) => "search",
        Command::Stat(_) => "stat",
        Command::Mkdir(_) => "mkdir",
        Command::Put(_) => "put",
        Command::Get(_) => "get",
        Command::Cat(_) => "cat",
        Command::Archive(_) => "archive",
        Command::Mv(_) => "mv",
        Command::Rm(_) => "rm",
        Command::Keep(_) => "keep",
        Command::Bin(_) => "bin",
        Command::Versions { .. } => "versions",
        Command::Restore(_) => "restore",
        Command::History(_) => "history",
        Command::Logs { .. } => "logs",
        Command::Link { .. } => "link",
        Command::Share(_) => "share",
        Command::Unshare(_) => "unshare",
        Command::Expiry(_) => "expiry",
        Command::Shares { .. } => "shares",
        Command::Access(_) => "access",
        Command::Inbox(_) => "inbox",
        Command::Usage { .. } => "usage",
        Command::Storage(_) => "storage",
        Command::App(_) => "app",
        Command::Config(_) => "config",
        Command::Report { .. } => "report",
        Command::Daemon(_) => "daemon",
        Command::Docs { .. } => "docs",
        Command::Version => "version",
        Command::Env { .. } => "env",
        Command::System { .. } => "system",
    }
}
