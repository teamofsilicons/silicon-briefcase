//! The command grammar, as `briefcase --help` shows it.

use std::{path::PathBuf, str::FromStr};

use briefcase_client::{AccessRight, ActorRef, EncryptionMode};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use uuid::Uuid;

/// Work with Silicon Briefcase from the command line.
#[derive(Parser)]
#[command(
    name = "briefcase",
    version,
    about = "Silicon Briefcase from the command line: your files, for every Carbon and Silicon.",
    long_about = "Silicon Briefcase from the command line: browse, upload, download and share files.\n\n\
        Every Carbon and every Silicon has its own drive. A path names an entry in your own drive \
        (notes/plan.md); put @ and an account id in front to name another drive — one something was \
        shared from, or the drive of a Silicon you look after (@c:ada/plans, @si:scout/reports). \
        An entry's identifier and its permanent address (https://briefcase.teamofsilicons.com/c:ada/plans) \
        work too.\n\n\
        Sign in first:\n  \
        Carbons:   briefcase login                (shows a code to approve on the account site)\n  \
        Silicons:  silicon-accounts login --app briefcase -q | briefcase login --slt-stdin\n\n\
        Then:\n  \
        briefcase login status --json       who you are signed in as\n  \
        briefcase drives                    your drive and the drives of the Silicons you look after\n  \
        briefcase shared                    what others shared with you\n  \
        briefcase ls notes                  a folder in your drive\n  \
        briefcase put ./report.pdf notes    upload\n  \
        briefcase share notes/report.pdf c:ada --access read,update\n\n\
        Run `briefcase <command> --help` for what a command is for, how it combines with others, and its options.\n\
        State lives in $SILICON_HOME/.briefcase when SILICON_HOME is set, otherwise $HOME/.briefcase \
        (`briefcase config home <directory>` moves it). Silicon Apps installs and updates briefcase \
        (`silicon-apps install briefcase`).",
    after_help = "Offline guides: briefcase docs cli | client\nRepository: https://github.com/teamofsilicons/silicon-briefcase\nDocs: https://docs.briefcase.teamofsilicons.com/\nRust package: https://crates.io/crates/briefcase-client",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Options every command accepts.
    #[command(flatten)]
    pub global: GlobalArgs,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Options accepted by every command.
#[derive(Args, Clone, Debug)]
pub struct GlobalArgs {
    /// Briefcase API URL, for a local or private deployment.
    ///
    /// Default: the saved profile's, else <https://backend.briefcase.teamofsilicons.com/api/v1/>.
    /// A saved sign-in is never sent to another URL than the one it was made for.
    #[arg(long, global = true, env = "BRIEFCASE_URL", value_name = "URL")]
    pub url: Option<String>,

    /// Silicon Accounts URL to sign in at, for a local or private deployment.
    ///
    /// Default: the saved profile's, else <https://accounts.teamofsilicons.com>. Plain http is
    /// accepted only for this machine (`localhost`, `127.0.0.1`, `::1`).
    #[arg(long, global = true, env = "ACCOUNTS_URL", value_name = "URL")]
    pub accounts_url: Option<String>,

    /// Use this Briefcase access token instead of the saved sign-in (for scripts).
    ///
    /// It is never refreshed or saved. `briefcase login` saves a sign-in that refreshes itself.
    #[arg(
        long,
        global = true,
        env = "BRIEFCASE_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,

    /// Saved profile to use: one sign-in and deployment each (default: the current profile).
    #[arg(long, global = true, env = "BRIEFCASE_PROFILE", value_name = "NAME")]
    pub profile: Option<String>,

    /// Print JSON instead of a table; errors become JSON on standard error.
    #[arg(long, global = true)]
    pub json: bool,

    /// Skip the contract check this client makes before its first call.
    #[arg(long, global = true)]
    pub no_verify: bool,

    /// Retired: every account has its own drive; name another drive with @c:id or @si:id.
    #[arg(long, global = true, hide = true, value_name = "ORG")]
    pub org: Option<String>,

    /// Retired: testing environments no longer exist.
    #[arg(long, global = true, hide = true, value_name = "ENVIRONMENT")]
    pub test: Option<String>,

    /// Retired: testing environments no longer exist.
    #[arg(long, global = true, hide = true, value_name = "SECRET")]
    pub app_secret: Option<String>,
}

/// Everything the CLI can do.
#[derive(Subcommand)]
pub enum Command {
    /// Sign in: a device code for Carbons, a short-lived token for Silicons.
    #[command(
        long_about = "Sign in to Briefcase with your Silicon account and save the sign-in.\n\n\
        Carbons run `briefcase login` with nothing else: it prints a code and a page on the account \
        site, waits while you approve it there, and saves the sign-in (add --open to open the page). \
        Silicons never see a page: mint a short-lived token and hand it over on standard input:\n\n  \
        silicon-accounts login --app briefcase -q | briefcase login --slt-stdin\n\n\
        `briefcase login <SLT>` and `--slt <SLT>` take the token as an argument instead (it then shows \
        in shell history and process lists; prefer --slt-stdin). A short-lived token works once, for \
        two minutes, and only for Briefcase; a refused one is used up, so mint a fresh one.\n\n\
        The sign-in is saved for the profile (--profile, or --save-as to name a new one) in \
        credentials.json, readable only by you, and refreshes itself. Check it with \
        `briefcase login status --json`; end it with `briefcase logout`.\n\n\
        Examples:\n  briefcase login\n  briefcase login --open --label \"build machine\"\n  \
        silicon-accounts login --app briefcase -q | briefcase login --slt-stdin --save-as scout"
    )]
    Login(LoginArgs),
    /// End this profile's sign-in at Silicon Accounts and forget it.
    #[command(long_about = "End this profile's sign-in and forget its tokens.\n\n\
        The refresh token is revoked at Silicon Accounts, so it can never be used again; only this \
        sign-in ends (other machines and the website stay signed in). When Silicon Accounts cannot \
        be reached the sign-in is still forgotten here and the answer says it was not revoked.")]
    Logout,
    /// Show how Briefcase signs accounts in (app id, URLs), without signing in.
    #[command(
        long_about = "Show Briefcase's app id at Silicon Accounts, the Silicon Accounts and API URLs this \
        command uses, and how Carbons and Silicons sign in. Works signed out and offline.\n\n\
        Example:\n  briefcase accounts --json"
    )]
    Accounts,
    /// Retired name of `accounts`, kept for older Silicon runtimes.
    #[command(hide = true)]
    Iam,
    /// Show the profile, the saved sign-in, and whether the deployment's contract matches.
    Status,
    /// List your drive and the drives of the Silicons you look after.
    #[command(
        long_about = "List the drives you open with the owner's rights: your own and, for a Carbon, \
        one per Silicon you look after (you can read, add, change, delete and share everything in \
        them). Address one with @ and its owner's id: `briefcase ls @si:scout`.\n\n\
        What others shared with you is in `briefcase shared`."
    )]
    Drives,
    /// List what other accounts shared with you, grouped by whose drive it is in.
    Shared(SharedArgs),
    /// Choose who besides you and your custodian's Silicons may share with a Silicon.
    #[command(subcommand)]
    Allow(AllowCommand),
    /// List a folder: the top of your drive when no target is given.
    Ls(LsArgs),
    /// Filter a drive with the filter language.
    Find(FindArgs),
    /// Search filenames and document text in your drive, your Silicons' and what was shared with you.
    Search(SearchArgs),
    /// Show one entry and what you may do with it.
    Stat(TargetArgs),
    /// Create a folder.
    Mkdir(MkdirArgs),
    /// Upload one or more local files.
    Put(PutArgs),
    /// Download a file, or a folder as tar.zst.
    Get(GetArgs),
    /// Write a file's bytes to standard output.
    Cat(TargetArgs),
    /// List what an archive (zip, tar, tar.gz, tar.zst, 7z) holds without extracting it.
    Archive(ArchiveArgs),
    /// Rename an entry or move it within its drive.
    Mv(MvArgs),
    /// Move entries to the recoverable bin.
    #[command(
        long_about = "Move entries to their drive's bin, where they stay restorable for 45 days with \
        `briefcase bin restore <entry-id>`.\n\n\
        Only an entry's creator, its drive's owner and their custodians may delete; it is never shared. \
        A self-destructing file is the exception: removing it deletes it permanently, at once, and it \
        never enters the bin. Check with `briefcase stat <target>` (a `self-destructs` line) before \
        removing, and use `briefcase keep <target>` first if it should survive."
    )]
    Rm(RmArgs),
    /// Keep a self-destructing file: stop its timer so it is never deleted.
    #[command(
        long_about = "Keep a self-destructing file: stop its timer so it is never deleted.\n\n\
        A file uploaded with `briefcase put --self-destruct <DURATION>` is deleted permanently when its \
        timer runs out; it never enters the bin and cannot be restored. `keep` makes it an ordinary, \
        permanent file again. The file's creator, the drive's owner and the custodian of either may \
        keep it; anyone else is refused (exit code 4).\n\n\
        Find files whose timer is running with `briefcase find is:self-destruct`, and check one with \
        `briefcase stat <target>`. Keeping a file that has no running timer is refused with \
        `not_self_destructing`, which is also what a retry after a lost response sees.\n\n\
        Example:\n  briefcase keep notes/draft.md"
    )]
    Keep(KeepArgs),
    /// Work with a drive's recoverable bin.
    #[command(subcommand)]
    Bin(BinCommand),
    /// List a file's retained versions.
    Versions {
        /// File path or ID.
        target: Target,
        /// Continue through older retained versions.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Restore an older version of a file.
    Restore(RestoreArgs),
    /// Show an entry's recorded history.
    History(TargetArgs),
    /// Read the preceding 365 days of file or folder logs.
    Logs {
        /// File or folder path or ID.
        target: Target,
        /// Continue from an earlier page.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Read or change anyone-with-the-link access, permanent or expiring.
    #[command(
        long_about = "Read or change anyone-with-the-link access: anyone holding the link can view and \
        download the file or folder.\n\n\
        With no option, prints the entry's link setting as JSON, including `expires_at` when its own \
        link is an expiring link. `--enabled true` turns on a permanent link, `--enabled false` turns \
        the link off, and `--expires-after <DURATION>` turns on an expiring link that stops working \
        after 1 minute to 30 days. The entry's creator, its drive's owner and their custodians may \
        change it; the top of a drive, `apps/` and `apps/<app>/` never get a link.\n\n\
        An expiring link is read-only and its expiry is strict: the link stops working the moment the \
        time passes. While it is live, `--expires-after` again restarts the clock, `--enabled true` makes \
        it permanent, and `--enabled false` ends it early. A permanent link must be turned off before \
        it can become an expiring link (`link_already_permanent`).\n\n\
        Examples:\n  briefcase link notes/report.pdf --expires-after 2h\n  \
        briefcase link notes/report.pdf\n  briefcase link notes/report.pdf --enabled true"
    )]
    Link {
        /// File or folder path or ID.
        target: Target,
        /// Set the explicit link policy to true or false; omit to inspect.
        ///
        /// `true` turns on a permanent link, and makes a live expiring link permanent.
        /// `false` turns the link off, and ends an expiring link early.
        #[arg(long)]
        enabled: Option<bool>,
        /// Turn on a read-only expiring link that stops working after DURATION.
        ///
        /// DURATION is whole minutes (`90`) or a number with m, h or d (`90m`,
        /// `2h`, `7d`, `1d12h`), from 1 minute to 30 days (43200 minutes). On a
        /// live expiring link it restarts the clock from now.
        #[arg(long, value_name = "DURATION", conflicts_with = "enabled")]
        expires_after: Option<Lifetime>,
    },
    /// Share an entry with a Carbon or Silicon by id, permanently or for a while.
    #[command(
        long_about = "Share a file or folder with an account, named by its id (c:ada, si:scout). The \
        answer is the new share as JSON; its `id` is what `briefcase unshare` and `briefcase expiry` take.\n\n\
        The share applies at once (nothing to accept). The recipient gets a notification in \
        `briefcase inbox`, and an email when it shared its email with Briefcase. Rights are read, write \
        (add content) and update (change content); delete is never shared. Only the entry's creator, \
        its drive's owner and their custodians may share it.\n\n\
        A Silicon accepts shares only from its custodian, the custodian's other Silicons, and accounts \
        it (or its custodian) allowed with `briefcase allow add`; anything else is refused with \
        `silicon_not_open`. Anyone signed in may share with a Carbon.\n\n\
        With `--expires-after <DURATION>` the share is an expiring share: read-only, and gone 1 minute \
        to 30 days after it is created. When it ends only that access goes. Before then, \
        `briefcase expiry` extends, shortens or makes it permanent, and `briefcase unshare` ends it early. \
        For anyone-with-the-link access, use `briefcase link <target> --expires-after <DURATION>`.\n\n\
        Examples:\n  briefcase share notes c:ada --access read,write --inherit\n  \
        briefcase share notes/report.pdf si:scout --expires-after 2h\n  \
        briefcase share @si:scout/reports c:bob --access read --inherit"
    )]
    Share(ShareArgs),
    /// Revoke one share, including an expiring share before it ends.
    #[command(
        long_about = "Revoke one share, as `briefcase shares <target>` lists it.\n\n\
        This also ends an expiring share early. Revoking sends no notification. Only the revoked share \
        goes: access the account holds through another share, an inherited share, creating the entry, \
        or owning (or looking after) the drive still applies."
    )]
    Unshare(UnshareArgs),
    /// Extend, shorten, or make permanent a live expiring share.
    #[command(
        long_about = "Change a live expiring share, as `briefcase shares <target>` lists it (a share \
        with an `expires_at`).\n\n\
        `--expires-in <DURATION>` restarts the share's clock: it now ends DURATION from now, which \
        extends or shortens it (1 minute to 30 days). `--permanent` keeps the read access for good; \
        when the recipient already holds a permanent share on the entry, the expiring one folds into \
        it. To end it early, use `briefcase unshare <target> <grant-id>`.\n\n\
        A share that has already ended is gone and reads as not found (exit code 3). A permanent share \
        has no timer and is refused with `not_an_expiring_share`.\n\n\
        Examples:\n  briefcase expiry notes \"$GRANT_ID\" --expires-in 3d\n  \
        briefcase expiry notes \"$GRANT_ID\" --permanent"
    )]
    Expiry(ExpiryArgs),
    /// List the live shares on an entry; expiring shares carry `expires_at`.
    Shares {
        /// File/folder UUID or path.
        target: Target,
        /// Continue listing shares from an opaque cursor.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show what you may do with named entries.
    Access(AccessArgs),
    /// Read your notification inbox.
    Inbox(InboxArgs),
    /// Show what a drive is consuming: your own unless you name a Silicon's you look after.
    Usage {
        /// The drive, by its owner's id (`@si:scout` or `si:scout`); your own when omitted.
        drive: Option<DriveArg>,
    },
    /// Bring your own S3 bucket for a drive's new files.
    #[command(subcommand)]
    Storage(StorageCommand),
    /// Act for an account as another app, with a Silicon Accounts proof.
    #[command(subcommand)]
    App(AppCommand),
    /// Stored CLI settings and the state directory.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Submit a bug report, optionally linking a proposed fix.
    #[command(
        long_about = "Submit reproduction steps, expected behavior, and actual behavior. Nothing is attached automatically.\n\n\
        Example: briefcase report \"Upload fails when the filename contains a tab\" --pr https://github.com/teamofsilicons/silicon-briefcase/pull/42\n\n\
        Reports go to the deployment you are signed in to, as your account. A PR is optional."
    )]
    Report {
        /// Description and reproduction steps (1–16384 bytes).
        message: String,
        /// Optional Briefcase GitHub pull request URL.
        #[arg(long)]
        pr: Option<String>,
    },
    /// Remove the retired background service an older briefcase installed.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Read bundled usage or development documentation without network access.
    Docs {
        /// Guide name; omit to list the available guides.
        topic: Option<String>,
    },
    /// Show the client and deployment contract versions.
    Version,
    /// Retired: testing environments no longer exist.
    #[command(hide = true)]
    Env {
        /// Ignored.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Retired: Silicon Apps installs and updates briefcase.
    #[command(hide = true)]
    System {
        /// Ignored.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
}

/// Arguments for `login`.
#[derive(Args)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_precedence_over_arg = true
)]
pub struct LoginArgs {
    /// Inspect the current sign-in instead of signing in.
    #[command(subcommand)]
    pub command: Option<LoginCommand>,

    /// A short-lived token (slt_…) from `silicon-accounts login --app briefcase -q`.
    ///
    /// Without one, a Carbon signs in with a device code.
    #[arg(index = 1, value_name = "SLT", conflicts_with_all = ["slt", "slt_stdin"])]
    pub slt_positional: Option<String>,

    /// A short-lived token (slt_…); prefer --slt-stdin, which keeps it out of shell history.
    #[arg(long, value_name = "SLT", conflicts_with = "slt_stdin")]
    pub slt: Option<String>,

    /// Read the short-lived token from standard input.
    #[arg(long, conflicts_with = "slt")]
    pub slt_stdin: bool,

    /// Device sign-in: open the approval page in the browser as well as printing it.
    #[arg(long, conflicts_with_all = ["slt", "slt_stdin", "slt_positional"])]
    pub open: bool,

    /// Device sign-in: the name shown on the approval page and in your sessions list.
    ///
    /// Default: `briefcase CLI on <host name>`.
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["slt", "slt_stdin", "slt_positional"])]
    pub label: Option<String>,

    /// Device sign-in: details to share with Briefcase, space-separated.
    ///
    /// Default: `email` when Briefcase asks for it (invitation emails reach you only then).
    #[arg(long, value_name = "DETAILS", conflicts_with_all = ["slt", "slt_stdin", "slt_positional"])]
    pub scope: Option<String>,

    /// Save the sign-in under this profile name and make it the current profile.
    #[arg(long, value_name = "NAME")]
    pub save_as: Option<String>,
}

/// Sign-in inspection.
#[derive(Subcommand)]
pub enum LoginCommand {
    /// Show whether this profile is signed in, and as which Carbon or Silicon.
    #[command(
        long_about = "Show whether this profile is signed in, and as which Carbon or Silicon.\n\n\
        Signed out it answers {\"authenticated\":false} (with `reason` when there is one to give); \
        signed in it gives the account's uuid, id and kind and when the sign-in expires. Briefcase \
        verifies the sign-in (refreshing it when needed) unless --offline is given; `verified` says \
        which. With --json the exit code is always 0; without, 1 when signed out."
    )]
    Status {
        /// Read only the saved sign-in: no refresh, no network.
        #[arg(long)]
        offline: bool,
    },
}

/// Arguments for `shared`.
#[derive(Args, Debug)]
pub struct SharedArgs {
    /// Entries per page, 1 through 100.
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub limit: Option<u16>,

    /// Continue after an opaque cursor returned by an earlier page.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,

    /// Follow pages until everything shared with you is listed.
    #[arg(short, long)]
    pub all: bool,
}

/// A Silicon's allow list.
#[derive(Debug, Subcommand)]
pub enum AllowCommand {
    /// List the accounts allowed to share with a Silicon.
    List {
        /// The Silicon (a custodian names one of its Silicons); yourself when you are a Silicon.
        #[arg(long = "for", value_name = "SILICON")]
        silicon: Option<DriveArg>,
    },
    /// Allow an account to share with a Silicon.
    #[command(long_about = "Allow an account to share with a Silicon.\n\n\
        A Silicon acts on what it receives, so only its custodian, the custodian's other Silicons and \
        the accounts on this list can share with it. The Silicon itself or its custodian manages the list.\n\n\
        Examples:\n  briefcase allow add c:bob                    (a Silicon allowing c:bob)\n  \
        briefcase allow add c:bob --for si:scout     (a custodian, for its Silicon)")]
    Add {
        /// The account to allow, by id (c:bob, si:helper).
        account: Principal,
        /// The Silicon; yourself when you are a Silicon.
        #[arg(long = "for", value_name = "SILICON")]
        silicon: Option<DriveArg>,
    },
    /// Stop allowing an account; shares it already made stay until revoked.
    Remove {
        /// The account, by id.
        account: Principal,
        /// The Silicon; yourself when you are a Silicon.
        #[arg(long = "for", value_name = "SILICON")]
        silicon: Option<DriveArg>,
    },
}

/// Arguments for `ls`.
#[derive(Args, Debug)]
pub struct LsArgs {
    /// Folder to list (`notes`, `@si:scout/reports`, an ID); the top of your drive when omitted.
    pub target: Option<Target>,

    /// Show size, owner, and what you may do.
    #[arg(short, long)]
    pub long: bool,

    /// Entries per page, 1 through 100.
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub limit: Option<u16>,

    /// Continue after an opaque cursor returned by an earlier page.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,

    /// Follow pages until the folder is exhausted.
    #[arg(short, long)]
    pub all: bool,
}

/// Arguments for `find`.
#[derive(Args, Debug)]
pub struct FindArgs {
    /// Filter expression, such as `is:md location:'notes' after:01-01-2026`.
    ///
    /// Beyond file kinds and extensions, `is:expiring` matches entries with an active expiring
    /// share, `is:self-destruct` files whose self-destruct timer is running, and
    /// `from:c:ada` / `to:si:scout` / `for:@si:scout` name accounts by id.
    pub filter: String,

    /// Folder or drive to filter inside (`@si:scout`, `notes`); the top of your drive when omitted.
    #[arg(long = "in", value_name = "TARGET")]
    pub in_folder: Option<Target>,

    /// Entries per page, 1 through 100.
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub limit: Option<u16>,

    /// Continue after an opaque cursor returned by an earlier page.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,

    /// Follow pages until the results are exhausted.
    #[arg(short, long)]
    pub all: bool,
}

/// Arguments for `search`.
#[derive(Args, Debug)]
pub struct SearchArgs {
    /// What to look for, in filenames and extracted document text.
    pub query: String,

    /// Results to return, 1 through 20.
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub limit: Option<u8>,
}

/// One entry, addressed by path or identifier.
#[derive(Args, Debug)]
pub struct TargetArgs {
    /// Entry: `notes/report.pdf` (your drive), `@c:ada/plans/q3.md`, an ID, or a permanent address.
    pub target: Target,
}

/// Arguments for `mkdir`.
#[derive(Args, Debug)]
pub struct MkdirArgs {
    /// Path of the folder to create, such as `notes/2026` or `@si:scout/reports`.
    ///
    /// A single segment creates the folder at the top of the drive.
    pub path: Target,

    /// Share the new folder with an account as it is created, as `c:ada=read,write`.
    #[arg(long = "invite", value_name = "ACCOUNT=RIGHTS")]
    pub invites: Vec<Invitation>,
}

/// Arguments for `put`.
#[derive(Args, Debug)]
pub struct PutArgs {
    /// Local files to upload.
    #[arg(required = true, value_name = "LOCAL")]
    pub sources: Vec<PathBuf>,

    /// Destination folder: a path, `@c:ada/shared`, an ID, or `/` for the top of your drive.
    pub destination: Target,

    /// Store the file under this name instead of its local one.
    ///
    /// Only valid with a single source file.
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Media type to record; guessed from the extension when omitted.
    #[arg(long, value_name = "TYPE")]
    pub content_type: Option<String>,

    /// Delete each new file permanently DURATION after its upload finishes.
    ///
    /// DURATION is whole minutes (`90`) or a number with m, h or d (`90m`,
    /// `2h`, `7d`, `1d12h`), from 1 minute to 30 days (43200 minutes). Only a
    /// new file can self-destruct: a name that already exists in the folder is
    /// refused (`self_destruct_requires_new_file`) instead of becoming a new
    /// version. When the timer runs out the file is deleted for good and never
    /// enters the bin; removing it by hand earlier is also permanent. Later
    /// versions do not change the timer. `briefcase keep <target>` stops it.
    #[arg(long, value_name = "DURATION")]
    pub self_destruct: Option<Lifetime>,
}

/// Arguments for `get`.
#[derive(Args, Debug)]
pub struct GetArgs {
    /// File or folder to download.
    pub target: Target,

    /// Where to write it; the entry's own name in the working directory by default.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

/// Arguments for `archive`.
#[derive(Args, Debug)]
pub struct ArchiveArgs {
    /// The archive file.
    pub target: Target,

    /// List at most this many entries (1 to 10000).
    #[arg(short = 'n', long, value_name = "COUNT")]
    pub limit: Option<u32>,
}

/// Arguments for `mv`.
#[derive(Args, Debug)]
pub struct MvArgs {
    /// Entry to rename or move.
    pub target: Target,

    /// New path in the same drive, such as `notes/renamed.md` or `@si:scout/archive/old.md`.
    ///
    /// Entries move only within their drive; to copy into another drive, download and upload.
    pub destination: Target,
}

/// Arguments for `rm`.
#[derive(Args, Debug)]
pub struct RmArgs {
    /// Entries to move to the bin; a self-destructing file is deleted for good.
    #[arg(required = true)]
    pub targets: Vec<Target>,
}

/// Arguments for `keep`.
#[derive(Args, Debug)]
pub struct KeepArgs {
    /// Self-destructing files to keep.
    #[arg(required = true)]
    pub targets: Vec<Target>,
}

/// A drive's recoverable bin.
#[derive(Debug, Subcommand)]
pub enum BinCommand {
    /// List entries waiting in a drive's bin.
    List {
        /// The drive, by its owner's id; your own when omitted.
        #[arg(long, value_name = "DRIVE")]
        drive: Option<DriveArg>,
        /// Entries per page, 1 through 100.
        #[arg(short = 'n', long, value_name = "COUNT")]
        limit: Option<u16>,
        /// Continue after an opaque cursor returned by an earlier page.
        #[arg(long, value_name = "CURSOR")]
        cursor: Option<String>,
        /// Follow pages until the bin is exhausted.
        #[arg(short, long)]
        all: bool,
    },
    /// Restore an entry from the bin, with everything that was inside it.
    Restore {
        /// Entry identifier, as `briefcase bin list` shows it.
        entry_id: Uuid,
    },
}

/// Arguments for `restore`.
#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// File whose version to restore.
    pub target: Target,

    /// Version identifier, as `briefcase versions` shows it.
    pub version_id: Uuid,
}

/// Arguments for `share`.
#[derive(Args, Debug)]
pub struct ShareArgs {
    /// Entry to share.
    pub target: Target,

    /// Who to share with: an account id such as `c:ada` or `si:scout`.
    pub principal: Principal,

    /// Rights to convey, comma separated: read, write, update. Read is always included.
    #[arg(long, value_name = "RIGHTS", default_value = "read")]
    pub access: Rights,

    /// Extend the share to everything inside a folder.
    #[arg(long)]
    pub inherit: bool,

    /// Make this a read-only expiring share that ends after DURATION.
    ///
    /// DURATION is whole minutes (`90`) or a number with m, h or d (`90m`,
    /// `2h`, `7d`, `1d12h`), from 1 minute to 30 days (43200 minutes). An expiring
    /// share conveys read only, so `--access` must be omitted or `read`.
    #[arg(long, value_name = "DURATION")]
    pub expires_after: Option<Lifetime>,
}

/// Arguments for `unshare`.
#[derive(Args, Debug)]
pub struct UnshareArgs {
    /// Entry the share is on.
    pub target: Target,

    /// Share identifier, as `briefcase shares` shows it.
    pub grant_id: Uuid,
}

/// Arguments for `expiry`.
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("change").required(true).args(["expires_in", "permanent"])))]
pub struct ExpiryArgs {
    /// Entry the expiring share is on.
    pub target: Target,

    /// Identifier of the expiring share, as `briefcase shares` shows it.
    pub grant_id: Uuid,

    /// End the share DURATION from now instead, extending or shortening it.
    ///
    /// DURATION is whole minutes (`90`) or a number with m, h or d (`90m`,
    /// `2h`, `7d`, `1d12h`), from 1 minute to 30 days (43200 minutes).
    #[arg(long, value_name = "DURATION")]
    pub expires_in: Option<Lifetime>,

    /// Keep the read access for good; the share stops being an expiring share.
    #[arg(long)]
    pub permanent: bool,
}

/// Arguments for `access`.
#[derive(Args, Debug)]
pub struct AccessArgs {
    /// Entries to report on, up to a hundred, all in one drive.
    #[arg(required = true)]
    pub targets: Vec<Target>,
}

/// Arguments for `inbox`.
#[derive(Args, Debug)]
pub struct InboxArgs {
    /// Mark every notification read.
    #[arg(long)]
    pub read: bool,
}

/// Bring-your-own storage.
#[derive(Debug, Subcommand)]
pub enum StorageCommand {
    /// Point a drive's new files at an S3 bucket its owner brings.
    #[command(
        long_about = "Point a drive's new files at an S3 bucket its owner brings.\n\n\
        Briefcase assumes the role and probes create, read, update and delete; the bucket becomes \
        active only when every check passes, and a failed probe keeps the previous configuration. \
        Existing versions stay where they are. Only the drive's owner (or the custodian of the \
        Silicon that owns it) may configure it."
    )]
    Configure(StorageConfigureArgs),
}

/// Arguments for `storage configure`.
#[derive(Args, Debug)]
pub struct StorageConfigureArgs {
    /// The drive to configure, by its owner's id; your own when omitted.
    #[arg(long, value_name = "DRIVE")]
    pub drive: Option<DriveArg>,
    /// Reuse this UUID with the exact same configuration after a lost response.
    /// A completed failed probe needs a new UUID to run the checks again.
    #[arg(long)]
    pub operation_id: Option<Uuid>,
    /// Bucket name.
    #[arg(long)]
    pub bucket: String,
    /// AWS region.
    #[arg(long)]
    pub region: String,
    /// Role Briefcase assumes to reach the bucket.
    #[arg(long)]
    pub role_arn: String,
    /// Prefix inside the bucket.
    #[arg(long, default_value = "briefcase")]
    pub prefix: String,
    /// AWS account that owns the bucket.
    #[arg(long)]
    pub account: String,
    /// Required server-side encryption.
    #[arg(long, value_enum, default_value_t = EncryptionArg::SseS3)]
    pub encryption: EncryptionArg,
    /// KMS key, required when encryption is `sse-kms`.
    #[arg(long)]
    pub kms_key_arn: Option<String>,
}

/// Calls another app makes for an account, with a Silicon Accounts proof.
#[derive(Subcommand)]
pub enum AppCommand {
    /// Prepare or send one delegated JSON operation with a User verification proof.
    ///
    /// An app acting for an account obtains a User verification proof from Silicon Accounts
    /// (receiving app `briefcase`, the operation's scope) and sends it here. First run with
    /// --describe to see the route, its scope and the body's SHA-256 without sending anything;
    /// then send it with the proof on standard input (--proof-stdin) or at the hidden prompt.
    /// Retrying a mutation keeps the same body and `operation_id`; nothing is retried
    /// automatically and no saved sign-in is used.
    Request(AppRequestArgs),
    /// Hash a local file and print a reservation manifest without contacting a server.
    PrepareUpload {
        /// Local file to hash; keep it unchanged until transfer completes.
        file: PathBuf,
        /// Stable logical upload UUID, retained for reserve, status, commit, and cancel.
        #[arg(long)]
        operation_id: Uuid,
        /// Existing destination folder path; empty selects the app's own folder.
        #[arg(long, default_value = "")]
        parent_path: String,
    },
    /// Transfer bytes into private staging; a separate commit with a proof publishes them.
    Transfer {
        /// Reservation UUID returned by upload-reserve.
        upload_id: Uuid,
        /// The same local file used to prepare the reservation manifest.
        file: PathBuf,
        /// Owner-only capability file saved by upload-reserve; prompted for when omitted.
        #[arg(long, conflicts_with = "capability_stdin")]
        capability_file: Option<PathBuf>,
        /// Read the private upload capability from standard input.
        #[arg(long, conflicts_with = "capability_file")]
        capability_stdin: bool,
    },
}

/// Exact-body delegated operation supported by `app request`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DelegatedOperation {
    /// Create one folder (scope `briefcase.folders.create`).
    FolderCreate,
    /// List one page (scope `briefcase.entries.list`); pagination inputs are part of the body.
    EntriesList,
    /// Read a file (scope `briefcase.files.read`), with an optional byte range in the body.
    FileRead,
    /// Move an entry the app created to the bin (scope `briefcase.entries.trash`).
    EntryTrash,
    /// Share an entry with an account (scope `briefcase.invitations.create`).
    ///
    /// Add `"expires_in_minutes"` (1 to 43200) inside `invitation` to make a
    /// read-only expiring share.
    Invite,
    /// Change anyone-with-the-link access (scope `briefcase.link_access.update`).
    ///
    /// Add `"expires_in_minutes"` (1 to 43200) with `"enabled": true` to turn
    /// on an expiring link.
    LinkAccess,
    /// Reserve a private upload and save its capability in a new private file (`briefcase.uploads.reserve`).
    UploadReserve,
    /// Publish staged bytes with the original logical UUID (`briefcase.uploads.commit`).
    UploadCommit,
    /// Reconcile an upload by its logical UUID (`briefcase.uploads.status`).
    UploadStatus,
    /// Cancel unpublished staging by its logical UUID (`briefcase.uploads.cancel`).
    UploadCancel,
}

/// Prepare or execute one delegated request, without retaining a proof.
#[derive(Args)]
pub struct AppRequestArgs {
    /// Operation whose exact JSON body is supplied below.
    #[arg(value_enum)]
    pub operation: DelegatedOperation,
    /// UTF-8 JSON file containing the operation inputs, not credentials.
    #[arg(long, value_name = "JSON_FILE")]
    pub body: PathBuf,
    /// Print the route, its proof scope and the canonical body without reading a proof or calling a server.
    #[arg(long, conflicts_with_all = ["proof", "proof_stdin", "output", "force", "capability_file"])]
    pub describe: bool,
    /// The proof (sap_…); prefer the hidden prompt or --proof-stdin over shell history.
    #[arg(long, conflicts_with = "proof_stdin")]
    pub proof: Option<String>,
    /// Read the proof from standard input.
    #[arg(long, conflicts_with = "proof")]
    pub proof_stdin: bool,
    /// Local destination for file-read; required when sending that operation.
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Atomically replace an existing file-read destination after complete delivery.
    #[arg(long, requires = "output")]
    pub force: bool,
    /// New owner-only file for upload-reserve's capability; never printed to standard output.
    #[arg(long, value_name = "NEW_FILE")]
    pub capability_file: Option<PathBuf>,
}

/// Stored CLI settings.
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show the current settings, profile and state directory.
    Show,
    /// Set the parent directory for the private `.briefcase` state directory.
    Home {
        /// Existing directory to use as the configured home.
        location: PathBuf,
    },
    /// Set a supported setting: `telemetry on|off`.
    Set {
        /// `telemetry`.
        key: String,
        /// `on` or `off`.
        value: String,
    },
    /// Restore a supported setting to its default.
    Unset {
        /// `telemetry`.
        key: String,
    },
}

/// The background service older releases installed.
#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Stop the retired background service and remove its login service (launchd/systemd).
    #[command(
        long_about = "Stop the background service an older briefcase installed and remove its login \
        service (a launchd agent on macOS, a systemd user unit on Linux). briefcase no longer runs \
        one: Silicon Apps keeps the CLI up to date. Safe to run when nothing is installed."
    )]
    Uninstall,
    /// What an old login service runs; it now removes that service and exits.
    #[command(hide = true)]
    Run,
    /// Retired.
    #[command(hide = true)]
    Start,
    /// Retired.
    #[command(hide = true)]
    Status,
    /// Retired.
    #[command(hide = true)]
    Stop,
    /// Retired.
    #[command(hide = true)]
    Install,
}

/// Server-side encryption for a bring-your-own bucket.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum EncryptionArg {
    /// S3-managed keys.
    SseS3,
    /// A customer-selected KMS key.
    SseKms,
}

impl From<EncryptionArg> for EncryptionMode {
    fn from(value: EncryptionArg) -> Self {
        match value {
            EncryptionArg::SseS3 => Self::SseS3,
            EncryptionArg::SseKms => Self::SseKms,
        }
    }
}

/// Whether a value names an account: a `c:`/`si:` id or a uuid.
pub fn is_account_reference(value: &str) -> bool {
    if let Some((kind, handle)) = value.split_once(':') {
        return matches!(kind, "c" | "si")
            && !handle.is_empty()
            && !handle.contains(['/', ':', '@'])
            && !handle.contains(char::is_whitespace);
    }
    (3..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// An entry named by identifier, by path inside a drive, or by an address
/// from before personal drives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A stable entry identifier.
    Id(Uuid),
    /// A path inside a drive (the caller's own when `drive` is absent); an empty
    /// path is the top of the drive.
    Path {
        /// The drive's owner, as a `c:`/`si:` id or uuid.
        drive: Option<String>,
        /// The path within the drive.
        path: String,
    },
    /// A permanent address from before personal drives (`/org/{org}/{path}`).
    Legacy {
        /// The old organization.
        organization: String,
        /// The path within it.
        path: String,
    },
}

impl Target {
    /// The drive this names, when it names one other than the caller's own.
    #[must_use]
    pub fn drive(&self) -> Option<&str> {
        match self {
            Self::Path { drive, .. } => drive.as_deref(),
            _ => None,
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(formatter, "{id}"),
            Self::Path {
                drive: Some(drive),
                path,
            } if path.is_empty() => write!(formatter, "@{drive}"),
            Self::Path {
                drive: Some(drive),
                path,
            } => write!(formatter, "@{drive}/{path}"),
            Self::Path { drive: None, path } if path.is_empty() => formatter.write_str("/"),
            Self::Path { drive: None, path } => formatter.write_str(path),
            Self::Legacy { organization, path } => write!(formatter, "/org/{organization}/{path}"),
        }
    }
}

const ADDRESS_FORMS: &str = "a path in your drive (notes/plan.md), @ and an account id for another drive \
    (@c:ada/plans), an entry ID, or a permanent address (https://briefcase.teamofsilicons.com/c:ada/plans)";

fn clean_path(path: &str) -> String {
    let path = path.trim();
    let path = path.strip_prefix("./").unwrap_or(path);
    path.trim_matches('/').to_owned()
}

impl FromStr for Target {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(format!("an entry is required: {ADDRESS_FORMS}"));
        }
        if let Ok(id) = Uuid::parse_str(trimmed) {
            return Ok(Self::Id(id));
        }
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            return parse_address(trimmed);
        }
        if let Some(rest) = trimmed.strip_prefix('@') {
            let (drive, path) = rest.split_once('/').unwrap_or((rest, ""));
            if !is_account_reference(drive) {
                return Err(format!(
                    "`@{drive}` does not name an account: write @ and an id such as @c:ada or @si:scout ({ADDRESS_FORMS})"
                ));
            }
            return Ok(Self::Path {
                drive: Some(drive.to_owned()),
                path: clean_path(path),
            });
        }
        Ok(Self::Path {
            drive: None,
            path: clean_path(trimmed),
        })
    }
}

/// The host of the hosted website, whose addresses are always Briefcase's.
const WEB_HOST: &str = "briefcase.teamofsilicons.com";

fn parse_address(value: &str) -> Result<Target, String> {
    let url = url::Url::parse(value)
        .map_err(|error| format!("`{value}` is not a URL ({error}); use {ADDRESS_FORMS}"))?;
    // On the hosted website or this machine every address form is Briefcase's.
    // Elsewhere only an owner's `c:`/`si:` id makes a URL unmistakably a
    // Briefcase address; a uuid-looking word (`/about`) could be anything.
    let known_host = match url.host() {
        Some(url::Host::Domain(domain)) => {
            domain.eq_ignore_ascii_case(WEB_HOST) || domain.eq_ignore_ascii_case("localhost")
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    let segments: Vec<String> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            percent_encoding::percent_decode_str(segment)
                .decode_utf8_lossy()
                .into_owned()
        })
        .collect();
    match segments.as_slice() {
        [org, organization, rest @ ..] if known_host && org == "org" => Ok(Target::Legacy {
            organization: organization.clone(),
            path: rest.join("/"),
        }),
        [owner, rest @ ..]
            if is_account_reference(owner) && (known_host || owner.contains(':')) =>
        {
            Ok(Target::Path {
                drive: Some(owner.clone()),
                path: rest.join("/"),
            })
        }
        _ => Err(format!(
            "`{value}` is not a Briefcase permanent address: those look like https://{WEB_HOST}/<owner id>/<path>; \
             for another deployment, write @<owner id>/<path>"
        )),
    }
}

/// A drive named by its owner: `@c:ada`, `si:scout`, or a uuid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriveArg(pub String);

impl FromStr for DriveArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        let id = trimmed
            .strip_prefix('@')
            .unwrap_or(trimmed)
            .trim_end_matches('/');
        if is_account_reference(id) {
            Ok(Self(id.to_owned()))
        } else {
            Err(format!(
                "`{trimmed}` does not name a drive: name it by its owner's id, such as @c:ada or @si:scout"
            ))
        }
    }
}

/// An account, as `c:ada` or `si:scout` (a leading `@` is accepted).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Principal(pub ActorRef);

impl FromStr for Principal {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        let id = trimmed.strip_prefix('@').unwrap_or(trimmed);
        if id.starts_with("email:") {
            return Err(
                "sharing by email address is retired: share with the account's id (c:<handle> or si:<handle>); \
                 the recipient gets an email when it shared its email with Briefcase"
                    .to_owned(),
            );
        }
        if id.starts_with("tag:") {
            return Err(
                "sharing with a tag is retired (accounts no longer have tags): share with each account by its id, \
                 such as c:ada or si:scout"
                    .to_owned(),
            );
        }
        ActorRef::from_id(id).map(Self).ok_or_else(|| {
            format!("`{trimmed}` is not an account id: write c:<handle> for a Carbon or si:<handle> for a Silicon")
        })
    }
}

/// A comma-separated set of rights.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rights(pub Vec<AccessRight>);

impl FromStr for Rights {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut rights = Vec::new();
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let right = AccessRight::parse(part)
                .ok_or_else(|| format!("{part} is not read, write or update"))?;
            if right == AccessRight::Delete {
                return Err(
                    "delete is never shared: only an entry's creator, its drive's owner and their custodians may delete"
                        .to_owned(),
                );
            }
            if !rights.contains(&right) {
                rights.push(right);
            }
        }
        if rights.is_empty() {
            return Err("at least one right is required".to_owned());
        }
        Ok(Self(rights))
    }
}

/// A share attached to a new folder, as `c:ada=read,write`.
#[derive(Clone, Debug)]
pub struct Invitation {
    /// Account being invited.
    pub principal: ActorRef,
    /// Rights they receive.
    pub access: Vec<AccessRight>,
}

impl FromStr for Invitation {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (principal, rights) = value.split_once('=').ok_or_else(|| {
            "a share is written account=rights, such as c:ada=read,write".to_owned()
        })?;
        Ok(Self {
            principal: principal.parse::<Principal>()?.0,
            access: rights.parse::<Rights>()?.0,
        })
    }
}

/// Longest lifetime an expiring share, expiring link or self-destruct timer may have:
/// 30 days, in minutes.
pub const MAXIMUM_LIFETIME_MINUTES: u32 = 43_200;

/// How to write a lifetime, repeated in every refusal so the fix is at hand.
const LIFETIME_FORMS: &str = "write whole minutes (`90`) or a number with m, h or d \
     (`90m`, `2h`, `7d`, `1d12h`), from 1 minute to 30 days (43200 minutes)";

/// How long an expiring share, expiring link or self-destructing file lasts, in whole
/// minutes.
///
/// Written as plain minutes (`90`) or with units (`90m`, `2h`, `7d`, `1d12h`).
/// The range is checked here, so an impossible lifetime never reaches the
/// server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lifetime(pub u32);

impl FromStr for Lifetime {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let text = value.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(format!("a lifetime is required; {LIFETIME_FORMS}"));
        }
        // Components saturate rather than overflow: anything that large is
        // refused as too long below, which is the truthful answer.
        let mut total = 0_u64;
        if text.bytes().all(|byte| byte.is_ascii_digit()) {
            total = text.parse().unwrap_or(u64::MAX);
        } else {
            let mut digits = String::new();
            let mut used = String::new();
            for character in text.chars() {
                let factor = match character {
                    '0'..='9' => {
                        digits.push(character);
                        continue;
                    }
                    ' ' => continue,
                    'd' => 24 * 60,
                    'h' => 60,
                    'm' => 1,
                    's' => {
                        return Err(format!(
                            "`{value}` uses seconds, but lifetimes are whole minutes; {LIFETIME_FORMS}"
                        ));
                    }
                    'w' => {
                        return Err(format!(
                            "`{value}` uses weeks; write days instead, such as `14d`; {LIFETIME_FORMS}"
                        ));
                    }
                    other => {
                        return Err(format!(
                            "`{other}` in `{value}` is not a unit; {LIFETIME_FORMS}"
                        ));
                    }
                };
                if digits.is_empty() {
                    return Err(format!(
                        "`{character}` in `{value}` has no number before it; {LIFETIME_FORMS}"
                    ));
                }
                if used.contains(character) {
                    return Err(format!(
                        "`{value}` uses `{character}` twice; {LIFETIME_FORMS}"
                    ));
                }
                used.push(character);
                let amount: u64 = digits.parse().unwrap_or(u64::MAX);
                total = total.saturating_add(amount.saturating_mul(factor));
                digits.clear();
            }
            if !digits.is_empty() {
                return Err(format!(
                    "`{digits}` at the end of `{value}` has no unit; {LIFETIME_FORMS}"
                ));
            }
        }
        if total == 0 {
            return Err(format!(
                "`{value}` is zero; the shortest lifetime is 1 minute. {LIFETIME_FORMS}"
            ));
        }
        u32::try_from(total)
            .ok()
            .filter(|minutes| *minutes <= MAXIMUM_LIFETIME_MINUTES)
            .map(Self)
            .ok_or_else(|| {
                let length = if total == u64::MAX {
                    String::new()
                } else {
                    format!(" is {total} minutes, which")
                };
                format!(
                    "`{value}`{length} is longer than the 30-day maximum (43200 minutes); {LIFETIME_FORMS}"
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, DriveArg, Invitation, Lifetime, Principal, Rights, Target};
    use clap::{CommandFactory as _, Parser as _};
    use uuid::Uuid;

    fn path(drive: Option<&str>, path: &str) -> Target {
        Target::Path {
            drive: drive.map(ToOwned::to_owned),
            path: path.to_owned(),
        }
    }

    #[test]
    fn the_grammar_itself_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_target_is_an_id_a_path_in_a_drive_or_an_address() {
        let id = Uuid::now_v7();
        assert_eq!(id.to_string().parse::<Target>().unwrap(), Target::Id(id));
        assert_eq!(
            "notes/plan.md".parse::<Target>().unwrap(),
            path(None, "notes/plan.md")
        );
        // A leading slash is a habit, not an error; `/` alone is the top.
        assert_eq!("/notes/".parse::<Target>().unwrap(), path(None, "notes"));
        assert_eq!("/".parse::<Target>().unwrap(), path(None, ""));
        assert_eq!("./@home".parse::<Target>().unwrap(), path(None, "@home"));
        assert_eq!(
            "@c:ada/plans/q3.md".parse::<Target>().unwrap(),
            path(Some("c:ada"), "plans/q3.md")
        );
        assert_eq!(
            "@si:scout".parse::<Target>().unwrap(),
            path(Some("si:scout"), "")
        );
        assert_eq!(
            "@zQo/notes".parse::<Target>().unwrap(),
            path(Some("zQo"), "notes")
        );
        assert_eq!(
            "https://briefcase.teamofsilicons.com/c:ada/plans/Q3%20plan.md"
                .parse::<Target>()
                .unwrap(),
            path(Some("c:ada"), "plans/Q3 plan.md")
        );
        assert_eq!(
            "https://briefcase.teamofsilicons.com/org/tos/public/handbook.md"
                .parse::<Target>()
                .unwrap(),
            Target::Legacy {
                organization: "tos".to_owned(),
                path: "public/handbook.md".to_owned()
            }
        );
        // A uuid at the top of the hosted website or a local one is an owner too.
        assert_eq!(
            "https://briefcase.teamofsilicons.com/zQo/notes"
                .parse::<Target>()
                .unwrap(),
            path(Some("zQo"), "notes")
        );
        assert_eq!(
            "http://localhost:4100/zQo/notes".parse::<Target>().unwrap(),
            path(Some("zQo"), "notes")
        );
        // Another deployment's website: unmistakable only with a c:/si: id.
        assert_eq!(
            "https://files.example.com/si:scout/reports"
                .parse::<Target>()
                .unwrap(),
            path(Some("si:scout"), "reports")
        );
        assert!("   ".parse::<Target>().is_err());
        assert!("@email:a@b.c/x".parse::<Target>().is_err());
        assert!("https://example.com/about".parse::<Target>().is_err());
        assert!(
            "https://example.com/org/tos/handbook.md"
                .parse::<Target>()
                .is_err()
        );
        assert!(
            "https://example.com/about"
                .parse::<Target>()
                .unwrap_err()
                .contains("@<owner id>/<path>")
        );
        for written in ["@c:ada/plans/q3.md", "@si:scout", "notes/a.md", "/"] {
            assert_eq!(written.parse::<Target>().unwrap().to_string(), written);
        }
    }

    #[test]
    fn drives_are_named_by_their_owners() {
        assert_eq!("@si:scout".parse::<DriveArg>().unwrap().0, "si:scout");
        assert_eq!("c:ada/".parse::<DriveArg>().unwrap().0, "c:ada");
        assert_eq!("zQo".parse::<DriveArg>().unwrap().0, "zQo");
        assert!("tos-org/x".parse::<DriveArg>().is_err());
        assert!("x".parse::<DriveArg>().is_err());
    }

    #[test]
    fn an_account_keeps_the_colon_in_its_id_and_retired_recipients_say_why() {
        let principal: Principal = "c:ada".parse().unwrap();
        assert_eq!(principal.0.id, "c:ada");
        assert_eq!(principal.0.actor_type.as_str(), "carbon");
        assert_eq!("@si:scout".parse::<Principal>().unwrap().0.id, "si:scout");
        assert!("person:cos".parse::<Principal>().is_err());
        assert!(
            "email:alex@example.com"
                .parse::<Principal>()
                .unwrap_err()
                .contains("retired")
        );
        assert!(
            "tag:engineering"
                .parse::<Principal>()
                .unwrap_err()
                .contains("retired")
        );
    }

    #[test]
    fn rights_parse_as_a_set_and_never_include_delete() {
        let rights: Rights = "read, write ,read".parse().unwrap();
        assert_eq!(rights.0.len(), 2);
        assert!("manage".parse::<Rights>().is_err());
        assert!(",".parse::<Rights>().is_err());
        assert!(
            "read,delete"
                .parse::<Rights>()
                .unwrap_err()
                .contains("never shared")
        );
    }

    #[test]
    fn an_invitation_carries_an_account_and_its_rights() {
        let invitation: Invitation = "si:scout=read,update".parse().unwrap();
        assert_eq!(invitation.principal.id, "si:scout");
        assert_eq!(invitation.access.len(), 2);
        assert!("si:scout".parse::<Invitation>().is_err());
    }

    #[test]
    fn retired_commands_and_options_still_parse_so_they_can_say_what_replaced_them() {
        for args in [
            vec!["briefcase", "--org", "tos", "ls"],
            vec![
                "briefcase",
                "--test",
                "01a067ce-7f19-7790-820a-0be6b3d4f800",
                "ls",
            ],
            vec!["briefcase", "env", "list"],
            vec!["briefcase", "system", "update"],
            vec!["briefcase", "iam", "--json"],
            vec!["briefcase", "daemon", "run"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
        for args in [
            vec!["briefcase", "request", "notes.md"],
            vec![
                "briefcase",
                "decide",
                "01a067ce-7f19-7790-820a-0be6b3d4f850",
                "approve",
            ],
        ] {
            let error = Cli::try_parse_from(args).err().unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
        }
    }

    #[test]
    fn login_takes_a_short_lived_token_three_ways_or_none() {
        let positional = Cli::try_parse_from(["briefcase", "login", "slt_abc"]).unwrap();
        let super::Command::Login(args) = positional.command else {
            panic!("expected login");
        };
        assert_eq!(args.slt_positional.as_deref(), Some("slt_abc"));
        let status = Cli::try_parse_from(["briefcase", "login", "status", "--offline"]).unwrap();
        let super::Command::Login(args) = status.command else {
            panic!("expected login");
        };
        assert!(matches!(
            args.command,
            Some(super::LoginCommand::Status { offline: true })
        ));
        assert!(Cli::try_parse_from(["briefcase", "login", "--slt-stdin"]).is_ok());
        assert!(Cli::try_parse_from(["briefcase", "login", "--open", "--label", "laptop"]).is_ok());
        assert!(Cli::try_parse_from(["briefcase", "login", "--slt", "a", "--slt-stdin"]).is_err());
        assert!(Cli::try_parse_from(["briefcase", "login", "slt_a", "--open"]).is_err());
    }

    #[test]
    fn common_invocations_parse() {
        let cli = Cli::try_parse_from(["briefcase", "ls", "@si:scout/reports", "--long"]).unwrap();
        assert!(matches!(cli.command, super::Command::Ls(_)));

        let cli =
            Cli::try_parse_from(["briefcase", "put", "a.txt", "b.txt", "@c:ada/shared"]).unwrap();
        let super::Command::Put(args) = cli.command else {
            panic!("expected put");
        };
        assert_eq!(args.sources.len(), 2);
        assert_eq!(args.destination, path(Some("c:ada"), "shared"));
        assert!(Cli::try_parse_from(["briefcase", "put", "only-one-argument"]).is_err());

        let cli = Cli::try_parse_from(["briefcase", "usage", "@si:scout"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Usage { drive: Some(_) }
        ));
        let cli = Cli::try_parse_from(["briefcase", "allow", "add", "c:bob", "--for", "si:scout"])
            .unwrap();
        assert!(matches!(cli.command, super::Command::Allow(_)));
        let cli =
            Cli::try_parse_from(["briefcase", "find", "is:pdf", "--in", "@si:scout"]).unwrap();
        assert!(matches!(cli.command, super::Command::Find(_)));
    }

    #[test]
    fn every_paginated_command_accepts_a_cursor_and_exhaustive_walk() {
        for args in [
            vec!["briefcase", "ls", "notes", "--cursor", "next", "--all"],
            vec!["briefcase", "find", "is:md", "--cursor", "next", "--all"],
            vec!["briefcase", "bin", "list", "--cursor", "next", "--all"],
            vec!["briefcase", "shared", "--cursor", "next", "--all"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn lifetimes_read_as_minutes_or_with_units() {
        for (written, minutes) in [
            ("1", 1),
            ("90", 90),
            ("90m", 90),
            ("2h", 120),
            ("7d", 10_080),
            ("30d", 43_200),
            ("43200", 43_200),
            ("1d12h", 2_160),
            ("1d 12h 30m", 2_190),
            ("2H", 120),
            (" 45m ", 45),
            ("0d1m", 1),
            ("29d23h60m", 43_200),
        ] {
            assert_eq!(
                written.parse::<Lifetime>(),
                Ok(Lifetime(minutes)),
                "{written}"
            );
        }
    }

    #[test]
    fn lifetimes_outside_one_minute_to_thirty_days_are_refused_with_the_range() {
        for written in [
            "0",
            "0m",
            "0d0h",
            "43201",
            "30d1m",
            "31d",
            "721h",
            "99999999999999999999",
        ] {
            let error = written.parse::<Lifetime>().unwrap_err();
            assert!(error.contains("1 minute"), "{written}: {error}");
            assert!(error.contains("43200 minutes"), "{written}: {error}");
        }
        let error = "45d".parse::<Lifetime>().unwrap_err();
        assert!(error.contains("64800 minutes"), "{error}");
        assert!(error.contains("30-day maximum"), "{error}");
    }

    #[test]
    fn malformed_lifetimes_say_what_is_wrong_and_show_examples() {
        for (written, reason) in [
            ("", "required"),
            ("30s", "seconds"),
            ("2w", "weeks"),
            ("2x", "not a unit"),
            ("h", "no number"),
            ("1h30", "no unit"),
            ("1h1h", "twice"),
            ("-5m", "not a unit"),
            ("1.5h", "not a unit"),
        ] {
            let error = written.parse::<Lifetime>().unwrap_err();
            assert!(error.contains(reason), "{written}: {error}");
            assert!(error.contains("`1d12h`"), "{written}: {error}");
        }
    }

    #[test]
    fn a_link_is_either_permanent_or_expiring_never_both() {
        let error = Cli::try_parse_from([
            "briefcase",
            "link",
            "notes/report.pdf",
            "--enabled",
            "true",
            "--expires-after",
            "2h",
        ])
        .err()
        .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn changing_an_expiring_share_takes_exactly_one_new_lifetime() {
        let grant = Uuid::from_u128(5).to_string();
        let cli =
            Cli::try_parse_from(["briefcase", "expiry", "notes", &grant, "--expires-in", "3d"])
                .unwrap();
        let super::Command::Expiry(args) = cli.command else {
            panic!("expected expiry");
        };
        assert_eq!(
            (args.expires_in, args.permanent),
            (Some(Lifetime(4_320)), false)
        );
        let neither = Cli::try_parse_from(["briefcase", "expiry", "notes", &grant])
            .err()
            .unwrap();
        assert_eq!(
            neither.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let both = Cli::try_parse_from([
            "briefcase",
            "expiry",
            "notes",
            &grant,
            "--expires-in",
            "1h",
            "--permanent",
        ])
        .err()
        .unwrap();
        assert_eq!(both.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn expiring_and_self_destruct_options_parse_into_minutes() {
        let cli = Cli::try_parse_from([
            "briefcase",
            "share",
            "notes",
            "c:alex",
            "--expires-after",
            "2h",
        ])
        .unwrap();
        let super::Command::Share(args) = cli.command else {
            panic!("expected share");
        };
        assert_eq!(args.expires_after, Some(Lifetime(120)));
        let error = Cli::try_parse_from([
            "briefcase",
            "put",
            "a.md",
            "notes",
            "--self-destruct",
            "31d",
        ])
        .err()
        .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }
}
