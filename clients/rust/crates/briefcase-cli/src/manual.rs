//! Documentation included in the published binary; no network or login needed.
use crate::{render::Output, run::CliError};

const GUIDES: &[(&str, &str)] = &[
    ("cli", include_str!("../manual/cli.md")),
    ("client", include_str!("../manual/client.md")),
];

pub fn show(topic: Option<&str>, output: Output) -> Result<(), CliError> {
    if let Some(topic) = topic {
        let (_, content) = GUIDES
            .iter()
            .find(|(name, _)| *name == topic)
            .ok_or_else(|| {
                CliError::Usage(format!("unknown guide `{topic}`; available: cli, client"))
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
        "login" | "logout" | "accounts" | "status" => {
            "Carbons sign in with `briefcase login` (approve the code it shows on the account site); Silicons with `silicon-accounts login --app briefcase -q | briefcase login --slt-stdin`. Check who you are with `briefcase login status --json` before changing files; `briefcase logout` ends the sign-in. `briefcase accounts --json` shows the app id and URLs without signing in."
        }
        "drives" | "shared" | "allow" => {
            "Every Carbon and Silicon has its own drive. `briefcase drives` lists yours and the drives of the Silicons you look after; `briefcase shared` lists what others shared with you. Address another drive with @ and its owner's id: `briefcase ls @si:scout`. A Silicon accepts shares only from its custodian, the custodian's other Silicons, and the accounts `briefcase allow add` lets in."
        }
        "ls" | "find" | "search" => {
            "Discover an entry, then use `briefcase stat <path-or-id>` to inspect it. Paths are in your own drive unless they start with @<id>/ (`@c:ada/plans`). Use --json for scripts and --help for filtering and pagination options."
        }
        "stat" | "access" => {
            "Inspect the entry and your effective rights before changing it. Use `briefcase ls` to find a path or identifier, then `briefcase get <target>` to download it."
        }
        "mkdir" | "put" => {
            "Create a folder with `briefcase mkdir --help`, then upload with `briefcase put --help` (`/` is the top of your drive, `@si:scout/...` a Silicon's you look after). Verify the result with `briefcase stat <target>` and the drive's allowance with `briefcase usage`. A new file uploaded with `--self-destruct <DURATION>` is deleted for good when its timer runs out; `briefcase keep <target>` stops the timer."
        }
        "keep" => {
            "Find self-destructing files with `briefcase find is:self-destruct` and check one with `briefcase stat <target>`. A self-destructing file never enters the bin, whether its timer runs out or it is removed by hand, so keep it before either happens."
        }
        "get" | "cat" | "archive" => {
            "Use `briefcase ls` to find a file. `briefcase cat <target>` streams its bytes to stdout, `briefcase archive <target>` lists an archive without extracting it, and `briefcase get --help` explains file and folder downloads."
        }
        "mv" | "rm" | "bin" => {
            "Inspect the target with `briefcase stat <target>` first. Entries move only within their drive. Deletions stay recoverable in the drive's bin for 45 days (`briefcase bin --help`); a self-destructing file is the exception: removing it is permanent."
        }
        "versions" | "restore" | "history" | "logs" => {
            "Use `briefcase versions <target>` to find a retained version and `briefcase restore --help` to roll back. History and logs explain who changed an entry and when."
        }
        "link" | "share" | "unshare" | "shares" | "expiry" => {
            "Inspect current access with `briefcase shares <target>` or `briefcase link <target>`. Share with an account by its id (c:ada, si:scout) and only the rights needed; delete is never shared. For temporary read access, add `--expires-after <DURATION>` (1 minute to 30 days) to `share` or `link`; change a live expiring share with `briefcase expiry`, end it early with `briefcase unshare`, and find entries carrying one with `briefcase find is:expiring`."
        }
        "inbox" => {
            "Read the notifications about shares made to you with `briefcase inbox`; `--read` marks them read. It is pulled: nothing runs in the background."
        }
        "usage" | "storage" => {
            "Use `briefcase usage --json` for a drive's exact byte counts (100 GiB of uploads a day and 1 PiB stored by default). A drive's owner, or the custodian of the Silicon that owns it, can bring an S3 bucket with `briefcase storage --help`; existing versions stay where they are."
        }
        "app" => {
            "For apps acting for an account with a Silicon Accounts User verification proof (receiving app `briefcase`, one scope per operation). Apps reach only apps/<app>/ in that account's drive unless Briefcase lists them as first-party. Read `briefcase docs client` before integrating."
        }
        "config" => {
            "Use `briefcase config show` to inspect saved profiles. `briefcase config home <directory>` moves the state directory. There are no other settings: Silicon Apps keeps briefcase up to date and Briefcase records no telemetry."
        }
        "daemon" | "version" => {
            "Silicon Apps installs and updates briefcase (`silicon-apps install briefcase`). An older release's background service is removed with `briefcase daemon uninstall`."
        }
        "report" => {
            "Include reproduction steps, expected behavior, and actual behavior. An optional --pr links a fix. Reports go to the deployment you are signed in to, as your account."
        }
        _ => {
            "Use `briefcase docs` to list the bundled usage and development guides. They are available without a network connection or sign-in."
        }
    }
}
