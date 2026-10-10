//! `briefcase` — the command-line client for Silicon Briefcase.
//!
//! Everything here is a thin, stateful shell around the `briefcase-client`
//! package: the package makes the calls (and signs accounts in at Silicon
//! Accounts), this binary remembers the sign-in, turns answers into something
//! readable, and gives a script an exit code it can branch on.
//!
//! Exit codes:
//!
//! - `0` the command did what it said,
//! - `1` something failed,
//! - `2` the command as typed cannot be carried out,
//! - `3` the entry was not found, or is not yours to see,
//! - `4` no usable sign-in, or the action is not allowed.

mod cli;
mod daemon;
mod error;
mod input;
mod manual;
mod render;
mod run;
mod session;
mod signin;
mod state;
mod telemetry;

#[tokio::main]
async fn main() -> std::process::ExitCode {
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
            return std::process::ExitCode::from(u8::try_from(code).unwrap_or(2));
        }
    };
    if std::env::var("BRIEFCASE_ORG").is_ok_and(|value| !value.trim().is_empty()) {
        eprintln!(
            "briefcase: BRIEFCASE_ORG is ignored: every Carbon and Silicon has its own drive now (name another drive with @c:<id>/… or @si:<id>/…)."
        );
    }
    let json = cli.global.json;
    let observation = telemetry::CommandObservation::new(&cli);
    let mut succeeded = false;
    let exit = match run::run(cli).await {
        Ok(()) => {
            succeeded = true;
            std::process::ExitCode::SUCCESS
        }
        Err(run::CliError::Exit(code)) => std::process::ExitCode::from(code),
        Err(error) => {
            if json {
                eprintln!("{}", error.to_json());
            } else {
                eprintln!("briefcase: {error}");
                if let Some(hint) = error.hint() {
                    eprintln!("hint: {hint}");
                }
            }
            std::process::ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1))
        }
    };
    observation.finish(succeeded).await;
    exit
}

/// Removes anything secret-looking from a parse error before printing it:
/// clap repeats the offending argument, which may be a token.
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
            "--token" | "--slt" | "--proof" | "--app-secret" | "--test"
        );
        let value = inline.unwrap_or(argument);
        if (secret_next
            || sensitive && inline.is_some()
            || [
                "slt_", "sar_", "sap_", "sapr_", "ask_", "oat_", "ort_", "stk", "eyJ",
            ]
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
