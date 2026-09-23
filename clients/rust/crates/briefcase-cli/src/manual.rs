//! Documentation included in the published binary; no network or login needed.
use crate::{render::Output, run::CliError};

const GUIDES: &[(&str, &str)] = &[
    ("cli", include_str!("../manual/cli.md")),
    ("client", include_str!("../manual/client.md")),
    ("testing", include_str!("../manual/testing.md")),
];

pub fn show(topic: Option<&str>, output: Output) -> Result<(), CliError> {
    if let Some(topic) = topic {
        let (_, content) = GUIDES
            .iter()
            .find(|(name, _)| *name == topic)
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "unknown guide `{topic}`; available: cli, client, testing"
                ))
            })?;
        if output.is_json() {
            output.json(&serde_json::json!({"topic": topic, "markdown": content}));
        } else {
            println!("{content}");
        }
    } else {
        output.json(&serde_json::json!({
            "guides": GUIDES.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            "usage": "briefcase docs <guide>",
            "repository": "https://github.com/teamofsilicons/silicon-briefcase",
            "documentation": "https://docs.briefcase.teamofsilicons.com/",
            "package": "https://crates.io/crates/briefcase-client"
        }));
    }
    Ok(())
}

/// Builds a documentation tree with workflow context at every command level.
pub fn parse() -> std::result::Result<crate::cli::Cli, clap::Error> {
    use clap::{CommandFactory as _, FromArgMatches as _};
    let mut command = crate::cli::Cli::command();
    for child in command.get_subcommands_mut() {
        let context = workflow(child.get_name());
        annotate(child, context);
    }
    crate::cli::Cli::from_arg_matches(&command.try_get_matches()?)
}

fn annotate(command: &mut clap::Command, context: &str) {
    *command = std::mem::take(command).after_help(format!("{context}\n\nRead the full offline guide with `briefcase docs cli`. Use --help on a subcommand to go deeper.\nRepository: https://github.com/teamofsilicons/silicon-briefcase\nDocs: https://docs.briefcase.teamofsilicons.com/\nRust package: https://crates.io/crates/briefcase-client"));
    for child in command.get_subcommands_mut() {
        annotate(child, context);
    }
}

fn workflow(command: &str) -> &'static str {
    match command {
        "iam" | "login" | "status" => {
            "Start with `briefcase iam --json`, obtain an IAM SLT, then `briefcase login <slt>`. Verify the identity with `briefcase login status --json` before changing files."
        }
        "logout" => {
            "Forget only the selected profile and environment session. Use `briefcase login status --json` afterwards to inspect authentication."
        }
        "ls" | "find" | "search" => {
            "Discover an entry, then use `briefcase stat <path-or-id>` to inspect it. Use --json for scripts and --help for filtering and pagination options."
        }
        "stat" | "access" => {
            "Inspect the entry and your effective rights before changing it. Use `briefcase ls` to find a path or identifier, then `briefcase get <target>` to download it."
        }
        "mkdir" | "put" => {
            "Create a folder with `briefcase mkdir --help`, then upload with `briefcase put --help`. Verify the resulting path with `briefcase stat <target>` and available space with `briefcase usage`."
        }
        "get" | "cat" => {
            "Use `briefcase ls` to find a file. `briefcase cat <target>` streams its bytes to stdout; `briefcase get --help` explains file and folder downloads."
        }
        "mv" | "rm" | "bin" => {
            "Inspect the target with `briefcase stat <target>` first. Deletions remain recoverable in `briefcase bin`; use `briefcase bin --help` for restoration."
        }
        "versions" | "restore" | "history" | "logs" => {
            "Use `briefcase versions <target>` to find a retained version and `briefcase restore --help` to roll back. History and logs explain who changed an entry and when."
        }
        "link" | "share" | "unshare" | "shares" => {
            "Inspect current access with `briefcase shares <target>` or `briefcase link <target>`. Grant only the required rights; use `briefcase access --help` to inspect effective permissions."
        }
        "inbox" => {
            "Read your permission-change notifications with `briefcase inbox`. This is a pulled inbox; no daemon WebSocket or outgoing webhook registration is required."
        }
        "usage" | "storage" => {
            "Use `briefcase usage --json` for exact byte counts. Organization owners can configure their own storage with `briefcase storage --help`; existing versions retain their original storage locations."
        }
        "env" => {
            "Create or import Briefcase in an IAM test environment, set BRIEFCASE_APP_SECRET, then `briefcase login <test-SLT-or-actor-ID>`. The same file commands now run with that test user's permissions. Read `briefcase docs testing` for lifecycle and limits."
        }
        "app" => {
            "Use these commands with an IAM OBO proof for an application acting as a member. The application namespace and the member's current rights both limit access. Read `briefcase docs client` before integrating."
        }
        "config" => {
            "Use `briefcase config show` to inspect saved settings. Independent updates are retired; Honeycomb manages releases. `briefcase config home <directory>` selects its state location."
        }
        "daemon" | "system" | "version" => {
            "Update with `honeycomb update 'briefcase'`. The optional daemon has no updater. Remove a service used only for updates with `briefcase daemon uninstall`."
        }
        "report" => {
            "Include reproduction steps, expected behavior, and actual behavior. An optional --pr links a fix. Reports use the selected organization and test environment."
        }
        _ => {
            "Use `briefcase docs` to list bundled usage, development, and testing guides. The guides are available without a network connection or login."
        }
    }
}
