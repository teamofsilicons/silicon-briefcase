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
mod render;
mod run;
mod state;
mod updater;

use clap::Parser as _;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();
    match updater::automatic(&cli.command).await {
        Ok(updater::Outcome::Updated { from, to }) => eprintln!(
            "briefcase: updated from {from} to {to}; the next invocation uses the new version"
        ),
        Ok(_) => {}
        Err(error) => eprintln!("briefcase: warning: automatic update skipped: {error}"),
    }
    match run::run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("briefcase: {error}");
            if let run::CliError::Client(briefcase_client::Error::Incompatible(_)) = &error {
                eprintln!(
                    "briefcase: upgrade the CLI, or pass --no-verify to call it anyway at your own risk"
                );
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            std::process::ExitCode::from(error.exit_code() as u8)
        }
    }
}
