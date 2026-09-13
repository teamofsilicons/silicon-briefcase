//! CLI-owned preferences; the SDK remains responsible for network delivery.
use crate::{
    cli::{Cli, Command},
    state::StateDirectory,
};
use briefcase_client::telemetry::{Event, Source, Stage};
use std::time::Instant;

pub fn enabled() -> bool {
    briefcase_client::telemetry::enabled_from_env()
        && StateDirectory::locate()
            .and_then(|state| state.configuration())
            .is_ok_and(|config| config.telemetry)
}

pub struct CommandObservation {
    started: Instant,
    event: Event,
    url: String,
    enabled: bool,
}
impl CommandObservation {
    pub fn new(cli: &Cli) -> Self {
        let mut event = Event::new(Source::Cli, name(&cli.command), Stage::Started);
        event.testing = cli.global.test.is_some() || cli.global.app_secret.is_some();
        event.environment_id = cli.global.test;
        let url = cli
            .global
            .url
            .clone()
            .or_else(|| {
                StateDirectory::locate()
                    .ok()?
                    .configuration()
                    .ok()
                    .and_then(|config| {
                        config
                            .profiles
                            .get(
                                cli.global
                                    .profile
                                    .as_deref()
                                    .unwrap_or(&config.current_profile),
                            )
                            .map(|profile| profile.url.clone())
                    })
            })
            .unwrap_or_else(|| "https://backend.briefcase.teamofsilicons.com/api/v1/".into());
        Self {
            started: Instant::now(),
            event,
            url,
            enabled: enabled() && !matches!(cli.command, Command::Docs { .. } | Command::Version),
        }
    }
    pub async fn finish(mut self, success: bool) {
        if !self.enabled || !enabled() {
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

pub async fn daemon_event(home: &StateDirectory, operation: &str, stage: Stage) {
    let Ok(config) = home.configuration() else {
        return;
    };
    if !config.telemetry || !briefcase_client::telemetry::enabled_from_env() {
        return;
    }
    let url = config.profiles.get(&config.current_profile).map_or(
        "https://backend.briefcase.teamofsilicons.com/api/v1/",
        |profile| profile.url.as_str(),
    );
    let _ = briefcase_client::telemetry::submit(url, &Event::new(Source::Daemon, operation, stage))
        .await;
}

fn name(command: &Command) -> &'static str {
    match command {
        Command::Login(_) => "login",
        Command::Iam => "iam",
        Command::Logout => "logout",
        Command::Status => "status",
        Command::Ls(_) => "ls",
        Command::Find(_) => "find",
        Command::Search(_) => "search",
        Command::Stat(_) => "stat",
        Command::Mkdir(_) => "mkdir",
        Command::Put(_) => "put",
        Command::Get(_) => "get",
        Command::Cat(_) => "cat",
        Command::Mv(_) => "mv",
        Command::Rm(_) => "rm",
        Command::Bin(_) => "bin",
        Command::Versions { .. } => "versions",
        Command::Restore(_) => "restore",
        Command::History(_) => "history",
        Command::Logs { .. } => "logs",
        Command::Link { .. } => "link",
        Command::Share(_) => "share",
        Command::Unshare(_) => "unshare",
        Command::Shares { .. } => "shares",
        Command::Access(_) => "access",
        Command::Inbox(_) => "inbox",
        Command::Usage => "usage",
        Command::Storage(_) => "storage",
        Command::App(_) => "app",
        Command::Env(_) => "env",
        Command::Config(_) => "config",
        Command::System(_) => "system",
        Command::Report { .. } => "report",
        Command::Daemon(_) => "daemon",
        Command::Docs { .. } => "docs",
        Command::Version => "version",
    }
}
