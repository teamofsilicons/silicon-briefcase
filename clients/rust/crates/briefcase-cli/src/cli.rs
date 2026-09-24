//! The command grammar, as `briefcase -h` shows it.

use std::{path::PathBuf, str::FromStr};

use briefcase_client::{
    AccessRight, ActorRef, ActorType, ApplicationId, Destination, EncryptionMode, RootType,
};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use uuid::Uuid;

/// Work with Silicon Briefcase from the command line.
#[derive(Parser)]
#[command(
    name = "briefcase",
    version,
    about = "Work with Silicon Briefcase from the command line.",
    long_about = "Browse, upload, download, and share organization files.\n\n\
                  Entries are addressed by the path their permanent URL shows, \
                  such as private/cos:tos/notes/report.pdf, or by their identifier. \
                  Run `briefcase login <slt>` once, then everything else uses the saved profile.\n\n\
                  Getting started:\n  briefcase iam --json\n  briefcase login <slt>\n  briefcase login status --json\n  briefcase --org tos ls\n\n\
                  Run `briefcase <command> --help` for full command usage and options.\n\
                  State defaults to $SILICON_HOME/.briefcase when SILICON_HOME is set,\n\
                  otherwise $HOME/.briefcase. Use `briefcase config home <directory>` to configure it.",
    after_help = "Offline guides: briefcase docs cli | client | testing\nRepository: https://github.com/teamofsilicons/silicon-briefcase\nDocs: https://docs.briefcase.teamofsilicons.com/\nRust package: https://crates.io/crates/briefcase-client",
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
#[derive(Args, Clone)]
pub struct GlobalArgs {
    /// Optional API URL override for local or private deployments.
    ///
    /// Uses the saved profile, or <https://backend.briefcase.teamofsilicons.com/api/v1/>
    /// automatically. Normal hosted use does not need this option.
    #[arg(long, global = true, env = "BRIEFCASE_URL", value_name = "URL")]
    pub url: Option<String>,

    /// Organization to act in, overriding the saved profile.
    #[arg(long, global = true, env = "BRIEFCASE_ORG", value_name = "ORG")]
    pub org: Option<String>,

    /// Run the same command inside a testing environment, by public UUID.
    #[arg(
        long,
        global = true,
        env = "BRIEFCASE_TEST",
        value_name = "ENVIRONMENT_ID",
        value_parser = parse_testing_environment_id
    )]
    pub test: Option<Uuid>,

    /// Select an IAM testing application directly by its secret.
    #[arg(
        long,
        global = true,
        env = "BRIEFCASE_APP_SECRET",
        hide_env_values = true,
        conflicts_with = "test"
    )]
    pub app_secret: Option<String>,

    /// IAM access token, overriding the saved one.
    #[arg(
        long,
        global = true,
        env = "BRIEFCASE_TOKEN",
        value_name = "TOKEN",
        hide_env_values = true
    )]
    pub token: Option<String>,

    /// Saved profile to use.
    #[arg(long, global = true, env = "BRIEFCASE_PROFILE", value_name = "NAME")]
    pub profile: Option<String>,

    /// Print JSON instead of a table.
    #[arg(long, global = true)]
    pub json: bool,

    /// Skip the contract check this client performs before its first call.
    #[arg(long, global = true)]
    pub no_verify: bool,
}

/// Everything the CLI can do.
#[derive(Subcommand)]
pub enum Command {
    /// Exchange an IAM short-lived token and save the rotating session.
    #[command(
        long_about = "Sign in to Silicon Briefcase with an IAM short-lived token.\n\n\
        The hosted backend is selected automatically; --url is only needed to override it \
        for a local or private deployment. Existing profiles keep their saved deployment.\n\n\
        Use `briefcase login status --json` to check authentication and identity.\n\n\
        Use `briefcase login <slt>` for a direct exchange, or omit the token to use the hidden prompt. `--org` is optional for normal login and is only needed when selecting a workspace or test plane.\n\n\
        In a test environment selected by --test or --app-secret, SLT can be an IAM-issued test login code or the existing test Carbon or Silicon ID, for example `alice` or `worker:tos`. IAM signs you in as that test actor."
    )]
    Login(LoginArgs),
    /// Show the deployment's public IAM app ID before signing in.
    Iam,
    /// Forget the saved session for this profile and plane.
    Logout,
    /// Show the current profile and whether the deployment agrees with it.
    Status,
    /// List a folder, or the organization base.
    Ls(LsArgs),
    /// Filter everything you can reach with the filter language.
    Find(FindArgs),
    /// Search filenames and extracted document text.
    Search(SearchArgs),
    /// Show one entry and what you may do with it.
    Stat(TargetArgs),
    /// Create a folder.
    Mkdir(MkdirArgs),
    /// Upload one or more local files.
    Put(PutArgs),
    /// Download a file or a folder as tar.zst.
    Get(GetArgs),
    /// Write a file's bytes to standard output.
    Cat(TargetArgs),
    /// Rename or move an entry.
    Mv(MvArgs),
    /// Move entries to the recoverable bin.
    #[command(
        long_about = "Move entries to the recoverable bin, where they stay restorable for 45 days \
        with `briefcase bin restore <entry-id>`.\n\n\
        A self-destructing file is the exception: removing it deletes it permanently, at once, \
        and it never enters the bin. Check with `briefcase stat <target>` (a `self-destructs` line) \
        before removing, and use `briefcase keep <target>` first if it should survive."
    )]
    Rm(RmArgs),
    /// Keep a self-destructing file: stop its timer so it is never deleted.
    #[command(
        long_about = "Keep a self-destructing file: stop its timer so it is never deleted.\n\n\
        A file uploaded with `briefcase put --self-destruct <DURATION>` is deleted permanently \
        when its timer runs out; it never enters the bin and cannot be restored. `keep` makes it \
        an ordinary, permanent file again. Only the file's creator and organization admins and \
        owners may keep it; anyone else is refused (exit code 4).\n\n\
        Find files whose timer is running with `briefcase find is:self-destruct`, and check one \
        with `briefcase stat <target>`. Keeping a file that has no running timer is refused with \
        `not_self_destructing`, which is also what a retry after a lost response sees.\n\n\
        Example:\n  briefcase keep private/si:cos/notes/draft.md"
    )]
    Keep(KeepArgs),
    /// Work with the recoverable bin.
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
    /// Read or change anyone-with-link access, permanent or expiring.
    #[command(
        long_about = "Read or change anyone-with-link access: anyone holding the link can view and \
        download the file or folder.\n\n\
        With no option, prints the entry's link setting as JSON, including `expires_at` when its \
        own link is an expiring link. `--enabled true` turns on a permanent link, `--enabled false` \
        turns the link off, and `--expires-after <DURATION>` turns on an expiring link that stops working after \
        1 minute to 30 days.\n\n\
        An expiring link is read-only and its expiry is strict: the link stops working the moment the \
        time passes. While it is live, `--expires-after` again restarts the clock (extend or shorten), \
        `--enabled true` makes it permanent, and `--enabled false` ends it early. Nobody is \
        notified when it ends; creating, changing and ending it are recorded in `briefcase logs`. \
        A permanent link must be turned off before it can become an expiring link \
        (`link_already_permanent`).\n\n\
        Examples:\n  briefcase link public/handbook/report.pdf --expires-after 2h\n  \
        briefcase link public/handbook/report.pdf\n  \
        briefcase link public/handbook/report.pdf --enabled true"
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
    /// Grant a member, contact or tag access to an entry, permanent or expiring.
    #[command(
        long_about = "Grant a member (c:ID, si:ID), a verified email contact (email:ADDRESS) or an \
        IAM tag (tag:TAG) access to a file or folder. The answer is the new grant as JSON; its \
        `id` is what `briefcase unshare` and `briefcase expiry` take.\n\n\
        With `--expires-after <DURATION>` the grant is an expiring share: read-only (view and download), and \
        gone 1 minute to 30 days after it is created. An expiring share is its own grant. When it ends, \
        only that access goes; access the recipient holds through another share, a tag, or a \
        public folder stays. Expiry is strict: access stops the moment the time passes. Before \
        then, `briefcase expiry` extends, shortens or makes it permanent, and `briefcase unshare` \
        ends it early. Nobody is notified when it ends; creating, changing and ending it are \
        recorded in `briefcase logs`. For anyone-with-the-link access, use \
        `briefcase link <target> --expires-after <DURATION>`.\n\n\
        Examples:\n  briefcase share private/si:cos/notes c:cos --access read,write --inherit\n  \
        briefcase share private/si:cos/notes/report.pdf email:alex@example.com --expires-after 2h\n  \
        briefcase share private/si:cos/notes tag:engineering --inherit --expires-after 7d"
    )]
    Share(ShareArgs),
    /// Revoke one grant, including an expiring share before it ends.
    #[command(
        long_about = "Revoke one grant, as `briefcase shares <target>` lists it.\n\n\
        This also ends an expiring share early. Revoking sends no notification to the recipient. \
        Only the revoked grant goes: access the recipient holds through another grant, a tag, \
        an inherited share or a public folder still applies."
    )]
    Unshare(UnshareArgs),
    /// Extend, shorten, or make permanent a live expiring share.
    #[command(
        long_about = "Change a live expiring share, as `briefcase shares <target>` lists it (a grant \
        with an `expires_at`).\n\n\
        `--expires-in <DURATION>` restarts the share's clock: it now ends DURATION from now, \
        which extends or shortens it (1 minute to 30 days). `--permanent` keeps the read access \
        for good; when the recipient already holds a permanent grant on the entry, the share \
        folds into it. To end it early, use `briefcase unshare <target> <grant-id>`. For an expiring \
        link, use `briefcase link <target> --expires-after <DURATION>` or `--enabled true` instead.\n\n\
        A share that has already ended is gone and reads as not found (exit code 3); create a new \
        one with `briefcase share <target> <recipient> --expires-after <DURATION>`. A permanent grant has \
        no timer and is refused with `not_an_expiring_share`. Changes are recorded in \
        `briefcase logs`; nobody is notified.\n\n\
        Examples:\n  briefcase expiry private/si:cos/notes \"$GRANT_ID\" --expires-in 3d\n  \
        briefcase expiry private/si:cos/notes \"$GRANT_ID\" --permanent"
    )]
    Expiry(ExpiryArgs),
    /// List the explicit grants on an entry; expiring shares carry `expires_at`.
    Shares {
        /// File/folder UUID or path.
        target: Target,
        /// Continue listing invitations from an opaque cursor.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Show what you may do with named entries.
    Access(AccessArgs),
    /// Read the notification inbox.
    Inbox(InboxArgs),
    /// Show what the organization is consuming.
    Usage,
    /// Configure organization-owned storage.
    #[command(subcommand)]
    Storage(StorageCommand),
    /// Act as an application, on behalf of a member.
    #[command(subcommand)]
    App(AppCommand),
    /// Disposable, IAM-coupled testing environments.
    #[command(subcommand)]
    Env(EnvCommand),
    /// Stored CLI settings.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Installation and update guidance.
    #[command(subcommand)]
    System(SystemCommand),
    /// Submit a bug report, optionally linking a proposed fix.
    #[command(
        long_about = "Submit reproduction steps, expected behavior, and actual behavior. Nothing is attached automatically.\n\nExample: briefcase report \"Upload fails when the filename contains a tab\" --pr https://github.com/teamofsilicons/silicon-briefcase/pull/42\n\nReports go to the selected deployment and test reports remain inside testing. A PR is optional."
    )]
    Report {
        /// Description and reproduction steps (1–16384 bytes).
        message: String,
        /// Optional Briefcase GitHub pull request URL.
        #[arg(long)]
        pr: Option<String>,
    },
    /// Manage the shared background service. Honeycomb manages updates.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Read bundled usage or development documentation without network access.
    Docs {
        /// Guide name; omit to list the available guides.
        topic: Option<String>,
    },
    /// Show the client and deployment contract versions.
    Version,
}

/// Arguments for `login`.
#[derive(Args)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_precedence_over_arg = true
)]
pub struct LoginArgs {
    /// Inspect the current login without exchanging a short-lived token.
    #[command(subcommand)]
    pub command: Option<LoginCommand>,

    /// IAM short-lived token; in testing, the Carbon or Silicon ID.
    #[arg(index = 1, value_name = "SLT", conflicts_with_all = ["slt", "slt_stdin"])]
    pub slt_positional: Option<String>,

    /// IAM SLT or test actor ID. Prompted for when omitted.
    #[arg(long, value_name = "SLT", conflicts_with = "slt_stdin")]
    pub slt: Option<String>,

    /// Read the IAM SLT or test actor ID from standard input.
    #[arg(long, conflicts_with = "slt")]
    pub slt_stdin: bool,

    /// Name to save this deployment under.
    #[arg(long, value_name = "NAME")]
    pub save_as: Option<String>,
}

/// Login inspection commands.
#[derive(Subcommand)]
pub enum LoginCommand {
    /// Verify the current session with IAM and show its Carbon or Silicon identity.
    Status,
}

/// Arguments for `ls`.
#[derive(Args, Debug)]
pub struct LsArgs {
    /// Folder to list; the organization base when omitted.
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
    /// Filter expression, such as `is:md location:'public' after:01-01-2026`.
    ///
    /// Beyond file kinds and extensions, `is:expiring` matches files and folders
    /// with an active expiring share (one that gives you access, or one you can
    /// manage), and `is:self-destruct` matches files whose self-destruct timer
    /// is still running.
    pub filter: String,

    /// Folder to filter inside; everything you can reach when omitted.
    #[arg(long, value_name = "TARGET")]
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
    /// Entry path, such as `private/cos:tos/notes/report.pdf`, or its identifier.
    pub target: Target,
}

/// Arguments for `mkdir`.
#[derive(Args, Debug)]
pub struct MkdirArgs {
    /// Path of the folder to create, such as `public/handbook/onboarding`.
    ///
    /// A single segment creates at the organization base, which needs `--type`.
    pub path: String,

    /// Access boundary for a folder created at the organization base.
    #[arg(long = "type", value_name = "KIND")]
    pub root_type: Option<RootTypeArg>,

    /// IAM tag, required when the type is `tag`.
    #[arg(long)]
    pub tag: Option<String>,

    /// Invite a member as the folder is created, as `c:cos=read,write`.
    #[arg(long = "invite", value_name = "PRINCIPAL=RIGHTS")]
    pub invites: Vec<Invitation>,
}

/// Arguments for `put`.
#[derive(Args, Debug)]
pub struct PutArgs {
    /// Local files to upload.
    #[arg(required = true, value_name = "LOCAL")]
    pub sources: Vec<PathBuf>,

    /// Destination folder, by path or identifier.
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
    /// File to download.
    pub target: Target,

    /// Where to write it; the file's own name in the working directory by default.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

/// Arguments for `mv`.
#[derive(Args, Debug)]
pub struct MvArgs {
    /// Entry to rename or move.
    pub target: Target,

    /// New path, such as `public/handbook/renamed.md`.
    pub destination: String,
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

/// The recoverable bin.
#[derive(Debug, Subcommand)]
pub enum BinCommand {
    /// List entries waiting in the bin.
    List {
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
    /// Restore an entry from the bin.
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

    /// Recipient: c:ID, si:ID, email:ADDRESS, or tag:TAG.
    pub principal: String,

    /// Rights to convey, comma separated. Read is always included.
    #[arg(long, value_name = "RIGHTS", default_value = "read")]
    pub access: Rights,

    /// Extend the grant to everything inside a folder.
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
    /// Entry the grant is on.
    pub target: Target,

    /// Grant identifier, as `briefcase shares` shows it.
    pub grant_id: Uuid,
}

/// Arguments for `expiring`.
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("change").required(true).args(["expires_in", "permanent"])))]
pub struct ExpiryArgs {
    /// Entry the expiring share is on.
    pub target: Target,

    /// Grant identifier of the expiring share, as `briefcase shares` shows it.
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
    /// Entries to report on, up to a hundred.
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

/// Organization storage.
#[derive(Debug, Subcommand)]
pub enum StorageCommand {
    /// Point the organization's files at a bucket it owns.
    Configure(StorageConfigureArgs),
}

/// Arguments for `storage configure`.
#[derive(Args, Debug)]
pub struct StorageConfigureArgs {
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

/// Application operations.
#[derive(Subcommand)]
pub enum AppCommand {
    /// Prepare or send one exact, IAM-authorized delegated JSON operation.
    ///
    /// First use --describe to see the endpoint and SHA-256 to give IAM.
    /// Mint a fresh proof, then repeat without --describe and paste the proof
    /// at the hidden prompt. Retrying a mutation requires the same body and
    /// `operation_id` but a new proof; no member login session is used.
    Request(AppRequestArgs),
    /// Hash a local file and print a reservation manifest without contacting a server.
    PrepareUpload {
        /// Local file to hash; keep it unchanged until transfer completes.
        file: PathBuf,
        /// Stable logical upload UUID, retained for reserve, status, commit, and cancel.
        #[arg(long)]
        operation_id: Uuid,
        /// Existing destination folder path; empty selects the member's private app folder.
        #[arg(long, default_value = "")]
        parent_path: String,
    },
    /// Transfer bytes into private staging; a separate fresh-proof commit publishes them.
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
    /// Create a file for the member an IAM proof represents.
    Upload {
        /// The application's IAM identifier.
        #[arg(long)]
        app_id: ApplicationId,
        /// The single-use proof minted over exactly these bytes. Prompted for when omitted.
        #[arg(long, conflicts_with = "proof_stdin")]
        proof: Option<String>,
        /// Read the single-use proof from standard input.
        #[arg(long, conflicts_with = "proof")]
        proof_stdin: bool,
        /// Local file whose bytes the proof was minted over.
        file: PathBuf,
    },
}

/// Exact-body delegated operation supported by `app request`.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DelegatedOperation {
    /// Create one folder under an existing path or the private app folder.
    FolderCreate,
    /// List one authorized page; pagination inputs are bound into the body.
    EntriesList,
    /// Read a file, with an optional body-bound byte range.
    FileRead,
    /// Move one entry to the recoverable bin using a stable operation UUID.
    EntryTrash,
    /// Invite a member using a critical IAM-approved proof.
    ///
    /// Add `"expires_in_minutes"` (1 to 43200) inside `invitation` to make a
    /// read-only expiring share.
    Invite,
    /// Change anonymous read access using a critical IAM-approved proof.
    ///
    /// Add `"expires_in_minutes"` (1 to 43200) with `"enabled": true` to turn
    /// on an expiring link.
    LinkAccess,
    /// Reserve an exact private upload and save its capability in a new private file.
    UploadReserve,
    /// Publish staged bytes using a fresh proof and the original logical UUID.
    UploadCommit,
    /// Reconcile an upload using its original logical UUID and fresh authority.
    UploadStatus,
    /// Cancel unpublished staging using its logical UUID and fresh authority.
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
    /// Print the canonical body and IAM binding without reading credentials or calling a server.
    #[arg(long, conflicts_with_all = ["app_id", "proof", "proof_stdin", "output", "force", "capability_file"])]
    pub describe: bool,
    /// Issuing application's canonical IAM identifier; required when sending.
    #[arg(long, required_unless_present = "describe")]
    pub app_id: Option<ApplicationId>,
    /// Single-use proof; prefer the hidden prompt or --proof-stdin over shell history.
    #[arg(long, conflicts_with = "proof_stdin")]
    pub proof: Option<String>,
    /// Read a freshly minted, single-use proof from standard input.
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

/// Testing-environment lifecycle and key-authorized self operations.
#[derive(Subcommand)]
pub enum EnvCommand {
    /// Manage shared environments through Honeycomb; arguments follow `honeycomb environments`.
    Manage {
        /// Honeycomb arguments, e.g. `create tos sandbox` or `action ID clean --revision 3`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// List active or recoverable environments.
    List {
        /// `active`, `deleted`, or `all`.
        #[arg(long)]
        status: Option<String>,
    },
    /// Provision paired IAM and Briefcase testing environments.
    Create {
        /// Human-readable environment name.
        name: String,
        /// Optional purpose or run description.
        #[arg(long)]
        description: Option<String>,
        /// Optional IAM root key for joining a dependency testing environment.
        #[arg(long, env = "BRIEFCASE_IAM_TEST_KEY", hide_env_values = true)]
        iam_test_key: Option<String>,
    },
    /// Show one environment without disclosing its key.
    Show {
        /// Public environment UUID.
        environment_id: Uuid,
    },
    /// Rename or re-describe an environment.
    Update {
        /// Public environment UUID.
        environment_id: Uuid,
        /// Replacement name.
        #[arg(long)]
        name: Option<String>,
        /// Replacement description.
        #[arg(long, conflicts_with = "clear_description")]
        description: Option<String>,
        /// Clear the current description.
        #[arg(long)]
        clear_description: bool,
    },
    /// Retire an environment for its recovery window.
    Delete {
        /// Public environment UUID.
        environment_id: Uuid,
    },
    /// Restore a retired environment before it is purged.
    Restore {
        /// Public environment UUID.
        environment_id: Uuid,
    },
    /// Retrieve and securely remember an environment's IAM app secret.
    Key {
        /// Public environment UUID.
        environment_id: Uuid,
    },
    /// Replace the paired IAM test plane without erasing Briefcase data.
    PairIam {
        /// Public Briefcase environment UUID.
        environment_id: Uuid,
        /// Public UUID from the replacement IAM testing environment.
        #[arg(long)]
        iam_environment_id: Uuid,
        /// Replacement IAM environment root key. Prompted for when omitted.
        #[arg(long, env = "BRIEFCASE_IAM_ENVIRONMENT_KEY", hide_env_values = true)]
        iam_environment_key: Option<String>,
        /// Canonical IAM ID of the imported Briefcase app (`org>handle`).
        #[arg(long)]
        iam_app_id: ApplicationId,
        /// Fresh test-only imported IAM Application secret. Prompted for when omitted.
        #[arg(long, env = "BRIEFCASE_IAM_APP_SECRET", hide_env_values = true)]
        iam_app_secret: Option<String>,
    },
    /// Erase disposable contents; omit the UUID to use the `--test` key.
    Clean {
        /// Managed environment UUID; omit for the selected `--test` plane.
        environment_id: Option<Uuid>,
    },
    /// Describe the selected testing environment using its IAM app secret.
    Current,
}

/// Stored CLI settings.
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show the current settings and profile.
    Show,
    /// Set the parent directory for the private `.briefcase` state directory.
    Home {
        /// Existing directory to use as the configured home.
        location: PathBuf,
    },
    /// Set a supported setting.
    Set {
        /// `auto-update` (off only) or `telemetry`.
        key: String,
        /// `on` or `off`.
        value: String,
    },
    /// Restore a supported setting to its default.
    Unset {
        /// `auto-update` (off only) or `telemetry`.
        key: String,
    },
}

/// Shared background service lifecycle.
#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Run in the foreground under a supervisor; registers this Silicon home.
    Run,
    /// Start a background process and register this Silicon home.
    Start,
    /// Show whether the daemon is responding and its registered homes.
    Status,
    /// Stop the shared daemon (a service supervisor may restart it).
    Stop,
    /// Install and start the current user's login service (macOS/Linux).
    Install,
    /// Uninstall the login service and stop its daemon.
    Uninstall,
}

/// Installed-CLI update guidance.
#[derive(Subcommand)]
pub enum SystemCommand {
    /// Show migration guidance for Honeycomb-managed updates.
    Update,
}

/// Accepts only a hyphenated UUID, never an IAM app secret.
fn parse_testing_environment_id(value: &str) -> Result<Uuid, String> {
    let bytes = value.as_bytes();
    let hyphenated = bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'));
    if !hyphenated {
        return Err(
            "expected a hyphenated testing-environment UUID, never its app secret".to_owned(),
        );
    }
    Uuid::parse_str(value).map_err(|_| "expected a valid testing-environment UUID".to_owned())
}

/// Access boundary for a user-created top-level folder.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RootTypeArg {
    /// Readable by every organization member.
    Public,
    /// Private to its owner unless explicitly shared.
    Private,
    /// Readable and writable by members of the specified IAM tag.
    Tag,
}

impl From<RootTypeArg> for RootType {
    fn from(value: RootTypeArg) -> Self {
        match value {
            RootTypeArg::Public => Self::Public,
            RootTypeArg::Private => Self::Private,
            RootTypeArg::Tag => Self::Tag,
        }
    }
}

/// Server-side encryption for an organization bucket.
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

/// An entry named by path or by identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// A stable entry identifier.
    Id(Uuid),
    /// An organization-relative path.
    Path(String),
}

impl Target {
    /// Returns the destination form the package accepts.
    #[must_use]
    pub fn destination(&self) -> Destination {
        match self {
            Self::Id(id) => Destination::Id(*id),
            Self::Path(path) => Destination::Path(path.clone()),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) => write!(formatter, "{id}"),
            Self::Path(path) => formatter.write_str(path),
        }
    }
}

impl FromStr for Target {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("an entry path or identifier is required".to_owned());
        }
        // A path may contain colons, so an identifier is recognized by shape
        // rather than by a prefix nobody would want to type.
        Uuid::parse_str(trimmed).map_or_else(
            |_| Ok(Self::Path(trimmed.trim_matches('/').to_owned())),
            |id| Ok(Self::Id(id)),
        )
    }
}

/// A member, as `c:saket` or `si:atlas`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Principal(pub ActorRef);

impl FromStr for Principal {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (prefix, handle) = value
            .split_once(':')
            .ok_or_else(|| "a member is written c:<handle> or si:<handle>".to_owned())?;
        let (actor_type, maximum) = match prefix {
            "c" => (ActorType::Carbon, 30),
            "si" => (ActorType::Silicon, 50),
            _ => return Err("a member is written c:<handle> or si:<handle>".to_owned()),
        };
        if !(3..=maximum).contains(&handle.len())
            || !handle.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            })
        {
            return Err("invalid member handle".to_owned());
        }
        Ok(Self(ActorRef {
            actor_type,
            id: value.to_owned(),
        }))
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
                .ok_or_else(|| format!("{part} is not read, write, update or delete"))?;
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

/// An invitation attached to a new folder, as `c:cos=read,write`.
#[derive(Clone, Debug)]
pub struct Invitation {
    /// Member being invited.
    pub principal: ActorRef,
    /// Rights they receive.
    pub access: Vec<AccessRight>,
}

impl FromStr for Invitation {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (principal, rights) = value.split_once('=').ok_or_else(|| {
            "an invitation is written principal=rights, such as c:cos=read,write".to_owned()
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
    use super::{Cli, Invitation, Lifetime, Principal, Rights, Target};
    use clap::{CommandFactory as _, Parser as _};
    use uuid::Uuid;

    #[test]
    fn the_grammar_itself_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn a_target_is_an_identifier_only_when_it_looks_like_one() {
        let id = Uuid::now_v7();
        assert_eq!(id.to_string().parse::<Target>().unwrap(), Target::Id(id));
        assert_eq!(
            "private/cos:tos/notes.md".parse::<Target>().unwrap(),
            Target::Path("private/cos:tos/notes.md".to_owned())
        );
        // A leading slash is a habit, not an error.
        assert_eq!(
            "/public/handbook".parse::<Target>().unwrap(),
            Target::Path("public/handbook".to_owned())
        );
        assert!("   ".parse::<Target>().is_err());
    }

    #[test]
    fn a_member_keeps_the_colons_in_their_own_identifier() {
        let principal: Principal = "c:cos".parse().unwrap();
        assert_eq!(principal.0.id, "c:cos");
        assert_eq!(principal.0.actor_type.as_str(), "carbon");
        assert!("person:cos:tos".parse::<Principal>().is_err());
        assert!("carbon".parse::<Principal>().is_err());
    }

    #[test]
    fn rights_parse_as_a_set_and_reject_nonsense() {
        let rights: Rights = "read, write ,read".parse().unwrap();
        assert_eq!(rights.0.len(), 2);
        assert!("manage".parse::<Rights>().is_err());
        assert!(",".parse::<Rights>().is_err());
    }

    #[test]
    fn an_invitation_carries_a_member_and_their_rights() {
        let invitation: Invitation = "si:atlas=read,update".parse().unwrap();
        assert_eq!(invitation.principal.id, "si:atlas");
        assert_eq!(invitation.access.len(), 2);
        assert!("si:atlas".parse::<Invitation>().is_err());
    }

    #[test]
    fn retired_access_request_commands_are_rejected() {
        for args in [
            vec!["briefcase", "request", "private/cos:owner/notes.md"],
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
    fn common_invocations_parse() {
        let cli = Cli::try_parse_from(["briefcase", "ls", "public", "--long"]).unwrap();
        assert!(matches!(cli.command, super::Command::Ls(_)));

        let cli =
            Cli::try_parse_from(["briefcase", "put", "a.txt", "b.txt", "public/handbook"]).unwrap();
        match cli.command {
            super::Command::Put(args) => {
                assert_eq!(args.sources.len(), 2);
                assert_eq!(args.destination, Target::Path("public/handbook".to_owned()));
            }
            _ => panic!("expected put"),
        }

        assert!(Cli::try_parse_from(["briefcase", "put", "only-one-argument"]).is_err());

        let id = Uuid::from_u128(8).to_string();
        let iam_id = Uuid::from_u128(9).to_string();
        let cli = Cli::try_parse_from([
            "briefcase",
            "env",
            "pair-iam",
            &id,
            "--iam-environment-id",
            &iam_id,
            "--iam-app-id",
            "briefcase",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Env(super::EnvCommand::PairIam { .. })
        ));
    }

    #[test]
    fn every_paginated_entry_command_accepts_a_cursor_and_exhaustive_walk() {
        let cli = Cli::try_parse_from([
            "briefcase",
            "ls",
            "public",
            "--cursor",
            "next-page",
            "--all",
        ])
        .unwrap();
        let super::Command::Ls(args) = cli.command else {
            panic!("expected ls");
        };
        assert_eq!(args.cursor.as_deref(), Some("next-page"));
        assert!(args.all);

        let cli = Cli::try_parse_from([
            "briefcase",
            "find",
            "is:md",
            "--cursor",
            "next-filter-page",
            "--all",
        ])
        .unwrap();
        let super::Command::Find(args) = cli.command else {
            panic!("expected find");
        };
        assert_eq!(args.cursor.as_deref(), Some("next-filter-page"));
        assert!(args.all);

        let cli = Cli::try_parse_from([
            "briefcase",
            "bin",
            "list",
            "--cursor",
            "next-bin-page",
            "--all",
        ])
        .unwrap();
        let super::Command::Bin(super::BinCommand::List { cursor, all, .. }) = cli.command else {
            panic!("expected bin list");
        };
        assert_eq!(cursor.as_deref(), Some("next-bin-page"));
        assert!(all);
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
    fn expiring_and_self_destruct_options_parse_into_minutes() {
        let cli = Cli::try_parse_from([
            "briefcase",
            "share",
            "private/cos:tos/notes",
            "email:alex@example.com",
            "--expires-after",
            "2h",
        ])
        .unwrap();
        let super::Command::Share(args) = cli.command else {
            panic!("expected share");
        };
        assert_eq!(args.expires_after, Some(Lifetime(120)));
        assert_eq!(
            args.access,
            Rights(vec![briefcase_client::AccessRight::Read])
        );

        let cli = Cli::try_parse_from([
            "briefcase",
            "link",
            "public/report.pdf",
            "--expires-after",
            "7d",
        ])
        .unwrap();
        let super::Command::Link {
            expires_after,
            enabled,
            ..
        } = cli.command
        else {
            panic!("expected link");
        };
        assert_eq!((expires_after, enabled), (Some(Lifetime(10_080)), None));

        let cli = Cli::try_parse_from([
            "briefcase",
            "put",
            "draft.md",
            "private/cos:tos",
            "--self-destruct",
            "90",
        ])
        .unwrap();
        let super::Command::Put(args) = cli.command else {
            panic!("expected put");
        };
        assert_eq!(args.self_destruct, Some(Lifetime(90)));

        let cli = Cli::try_parse_from([
            "briefcase",
            "keep",
            "private/cos:tos/a.md",
            "private/cos:tos/b.md",
        ])
        .unwrap();
        let super::Command::Keep(args) = cli.command else {
            panic!("expected keep");
        };
        assert_eq!(args.targets.len(), 2);
        assert!(Cli::try_parse_from(["briefcase", "keep"]).is_err());

        // A lifetime is checked while parsing, before any network call.
        let error = Cli::try_parse_from([
            "briefcase",
            "put",
            "a.md",
            "public",
            "--self-destruct",
            "31d",
        ])
        .err()
        .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn a_link_is_either_permanent_or_expiring_never_both() {
        let error = Cli::try_parse_from([
            "briefcase",
            "link",
            "public/report.pdf",
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
    fn changing_a_expiring_share_takes_exactly_one_new_lifetime() {
        let grant = Uuid::from_u128(5).to_string();
        let cli = Cli::try_parse_from([
            "briefcase",
            "expiry",
            "private/cos:tos/notes",
            &grant,
            "--expires-in",
            "3d",
        ])
        .unwrap();
        let super::Command::Expiry(args) = cli.command else {
            panic!("expected expiry");
        };
        assert_eq!(args.grant_id, Uuid::from_u128(5));
        assert_eq!(
            (args.expires_in, args.permanent),
            (Some(Lifetime(4_320)), false)
        );

        let cli = Cli::try_parse_from([
            "briefcase",
            "expiry",
            "private/cos:tos/notes",
            &grant,
            "--permanent",
        ])
        .unwrap();
        let super::Command::Expiry(args) = cli.command else {
            panic!("expected expiry");
        };
        assert_eq!((args.expires_in, args.permanent), (None, true));

        let neither = Cli::try_parse_from(["briefcase", "expiry", "private/cos:tos/notes", &grant])
            .err()
            .unwrap();
        assert_eq!(
            neither.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let both = Cli::try_parse_from([
            "briefcase",
            "expiry",
            "private/cos:tos/notes",
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
    fn test_context_accepts_only_a_public_hyphenated_uuid() {
        let id = Uuid::from_u128(7).to_string();
        let cli = Cli::try_parse_from(["briefcase", "--test", &id, "status"]).unwrap();
        assert_eq!(cli.global.test, Some(Uuid::from_u128(7)));
        assert!(Cli::try_parse_from(["briefcase", "--test", &"a".repeat(32), "status"]).is_err());
    }

    #[test]
    fn canonical_application_ids_are_enforced_by_the_grammar() {
        assert!(
            Cli::try_parse_from([
                "briefcase",
                "app",
                "upload",
                "--app-id",
                "notes",
                "--proof",
                "proof",
                "note.md",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "briefcase",
                "app",
                "upload",
                "--app-id",
                "tos>notes",
                "--proof",
                "proof",
                "note.md",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["briefcase", "app", "upload", "--app-id", "notes", "note.md",])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "briefcase",
                "app",
                "upload",
                "--app-id",
                "notes",
                "--proof-stdin",
                "note.md",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "briefcase",
                "app",
                "upload",
                "--app-id",
                "notes",
                "--proof",
                "proof",
                "--proof-stdin",
                "note.md",
            ])
            .is_err()
        );
    }
}
