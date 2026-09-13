//! `briefcase` — the command-line client for Silicon Briefcase.
//!
//! Everything here is a thin, stateful shell around the `briefcase-client`
//! package: the package makes the calls, this binary remembers which
//! deployment you meant, turns answers into something readable, and gives a
//! script an exit code it can branch on.
//!
//! Exit codes:
//!
//! - `0` the command did what it said,
//! - `1` something failed,
//! - `2` the command as typed cannot be carried out,
//! - `3` the entry was not found, or is not yours to see,
//! - `4` the credential was refused, or the action is not allowed.

mod cli;
mod daemon;
mod manual;
mod render;
mod run;
mod state;
mod telemetry;
mod updater;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let mut testing = testing_selection();
    let cli = match manual::parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let message = redact_parse_error(&error.to_string());
            if code == 0 {
                print!("{message}");
            } else {
                eprint!("{message}");
            }
            footer(testing.as_deref());
            return std::process::ExitCode::from(u8::try_from(code).unwrap_or(2));
        }
    };
    let register = !matches!(
        cli.command,
        cli::Command::Daemon(_) | cli::Command::Docs { .. }
    );
    let observation = telemetry::CommandObservation::new(&cli);
    let mut succeeded = false;
    let exit = match run::run(cli, &mut testing).await {
        Ok(()) => {
            succeeded = true;
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("briefcase: {error}");
            if let run::CliError::Client(briefcase_client::Error::Incompatible(_)) = &error {
                eprintln!(
                    "briefcase: use matching CLI and server versions; check `briefcase --version` and the deployment's `/api/version` before retrying"
                );
            }
            std::process::ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
        }
    };
    // Never delay exchange of one-use credentials for local maintenance.
    if register {
        let _ = daemon::register().await;
    }
    observation.finish(succeeded).await;
    footer(testing.as_deref());
    exit
}

fn footer(selection: Option<&str>) {
    if let Some(selection) = selection {
        eprintln!("TEST ENVIRONMENT — {}", render::terminal_text(selection));
    }
}

fn testing_selection() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selected = args.iter().any(|arg| {
        arg == "--test"
            || arg.starts_with("--test=")
            || arg == "--app-secret"
            || arg.starts_with("--app-secret=")
    }) || std::env::var_os("BRIEFCASE_TEST").is_some()
        || std::env::var_os("BRIEFCASE_APP_SECRET").is_some();
    // Never echo unparsed arguments: they may be secrets, even in a malformed
    // command. A successfully parsed selector replaces this with its UUID.
    selected.then(|| "test selection requested; environment has not been resolved".into())
}

fn redact_parse_error(message: &str) -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut result = message.to_owned();
    let mut secret_next = false;
    for argument in &args {
        let (flag, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let sensitive = matches!(
            flag,
            "--app-secret"
                | "--token"
                | "--slt"
                | "--iam-app-secret"
                | "--iam-test-key"
                | "--iam-environment-key"
        );
        let value = inline.unwrap_or(argument);
        if (secret_next
            || sensitive && inline.is_some()
            || ["ask_", "oat_", "ort_", "slt_"]
                .iter()
                .any(|prefix| value.starts_with(prefix)))
            && !value.is_empty()
        {
            result = result.replace(value, "<redacted>");
        }
        secret_next = sensitive && inline.is_none();
    }
    result
}
