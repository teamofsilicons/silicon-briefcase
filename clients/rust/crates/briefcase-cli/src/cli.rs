//! The command grammar, as `briefcase -h` shows it.

use std::{path::PathBuf, str::FromStr};

use briefcase_client::{
    AccessRight, ActorRef, ActorType, ApplicationId, Destination, EncryptionMode, RootType,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
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
                  Run `briefcase login` once, then everything else uses the saved profile.",
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
        Start with `briefcase login --org <organization>` and paste the IAM token at the hidden prompt."
    )]
    Login(LoginArgs),
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
    /// Download a file.
    Get(GetArgs),
    /// Write a file's bytes to standard output.
    Cat(TargetArgs),
    /// Rename or move an entry.
    Mv(MvArgs),
    /// Move entries to the recoverable bin.
    Rm(RmArgs),
    /// Work with the recoverable bin.
    #[command(subcommand)]
    Bin(BinCommand),
    /// List a file's retained versions.
    Versions(TargetArgs),
    /// Restore an older version of a file.
    Restore(RestoreArgs),
    /// Show an entry's recorded history.
    History(TargetArgs),
    /// Grant a member access to an entry.
    Share(ShareArgs),
    /// Revoke one grant.
    Unshare(UnshareArgs),
    /// List the explicit grants on an entry.
    Shares(TargetArgs),
    /// Show what you may do with named entries.
    Access(AccessArgs),
    /// Ask for access to an entry you cannot read.
    Request(RequestArgs),
    /// Approve or deny an access request.
    Decide(DecideArgs),
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
    /// Maintenance of the installed CLI.
    #[command(subcommand)]
    System(SystemCommand),
    /// Show the client and deployment contract versions.
    Version,
}

/// Arguments for `login`.
#[derive(Args)]
pub struct LoginArgs {
    /// IAM short-lived token. Prompted for when omitted.
    #[arg(long, value_name = "SLT", conflicts_with = "slt_stdin")]
    pub slt: Option<String>,

    /// Read the IAM short-lived token from standard input.
    #[arg(long, conflicts_with = "slt")]
    pub slt_stdin: bool,

    /// Name to save this deployment under.
    #[arg(long, value_name = "NAME")]
    pub save_as: Option<String>,
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

    /// Which container a base-level folder belongs in.
    #[arg(long = "type", value_name = "KIND")]
    pub root_type: Option<RootTypeArg>,

    /// IAM tag, required when the type is `tag`.
    #[arg(long)]
    pub tag: Option<String>,

    /// Invite a member as the folder is created, as `carbon:cos:tos=read,write`.
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
    /// Entries to move to the bin.
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

    /// Member to share it with, as `carbon:cos:tos` or `silicon:atlas`.
    pub principal: Principal,

    /// Rights to convey, comma separated. Read is always included.
    #[arg(long, value_name = "RIGHTS", default_value = "read")]
    pub access: Rights,

    /// Extend the grant to everything inside a folder.
    #[arg(long)]
    pub inherit: bool,
}

/// Arguments for `unshare`.
#[derive(Args, Debug)]
pub struct UnshareArgs {
    /// Entry the grant is on.
    pub target: Target,

    /// Grant identifier, as `briefcase shares` shows it.
    pub grant_id: Uuid,
}

/// Arguments for `access`.
#[derive(Args, Debug)]
pub struct AccessArgs {
    /// Entries to report on, up to a hundred.
    #[arg(required = true)]
    pub targets: Vec<Target>,
}

/// Arguments for `request`.
#[derive(Args, Debug)]
pub struct RequestArgs {
    /// Hidden permanent-URL path, or a known entry identifier, to ask about.
    pub target: Target,

    /// Rights to ask for, comma separated.
    #[arg(long, value_name = "RIGHTS", default_value = "read")]
    pub access: Rights,

    /// Why you need it.
    #[arg(long)]
    pub reason: Option<String>,
}

/// Arguments for `decide`.
#[derive(Args, Debug)]
pub struct DecideArgs {
    /// Request identifier, as the inbox shows it.
    pub request_id: Uuid,

    /// What to answer.
    #[arg(value_enum)]
    pub decision: DecisionArg,

    /// Rights to grant on approval, comma separated.
    #[arg(long, value_name = "RIGHTS", default_value = "read")]
    pub access: Rights,
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

/// Testing-environment lifecycle and key-authorized self operations.
#[derive(Subcommand)]
pub enum EnvCommand {
    /// List active or recoverable environments.
    List {
        /// `active`, `deleted`, or `all`.
        #[arg(long)]
        status: Option<String>,
    },
    /// Create a Briefcase plane coupled to an existing IAM test plane.
    Create {
        /// Human-readable environment name.
        name: String,
        /// Optional purpose or run description.
        #[arg(long)]
        description: Option<String>,
        /// Public UUID from `iam env create`.
        #[arg(long)]
        iam_environment_id: Uuid,
        /// IAM environment root key. Prompted for when omitted.
        #[arg(long, env = "BRIEFCASE_IAM_ENVIRONMENT_KEY", hide_env_values = true)]
        iam_environment_key: Option<String>,
        /// Canonical IAM ID of the imported Briefcase app (`org>handle`).
        #[arg(long)]
        iam_app_id: ApplicationId,
        /// Test-only imported IAM Application secret. Prompted for when omitted.
        #[arg(long, env = "BRIEFCASE_IAM_APP_SECRET", hide_env_values = true)]
        iam_app_secret: Option<String>,
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
    /// Retrieve and securely remember an environment root key.
    Key {
        /// Public environment UUID.
        environment_id: Uuid,
    },
    /// Rotate, print, and securely remember an environment root key.
    RotateKey {
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
    /// Describe the environment selected by `--test`, using only its root key.
    Current,
}

/// Stored CLI settings.
#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show the current updater policy and profile.
    Show,
    /// Set a supported setting.
    Set {
        /// Currently `auto-update`.
        key: String,
        /// `on` or `off`.
        value: String,
    },
    /// Restore a supported setting to its default.
    Unset {
        /// Currently `auto-update`.
        key: String,
    },
}

/// Installed-CLI maintenance.
#[derive(Subcommand)]
pub enum SystemCommand {
    /// Check crates.io now and install the latest CLI release.
    Update,
}

/// Accepts only a hyphenated UUID, never a 32-character root key.
fn parse_testing_environment_id(value: &str) -> Result<Uuid, String> {
    let bytes = value.as_bytes();
    let hyphenated = bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'));
    if !hyphenated {
        return Err(
            "expected a hyphenated testing-environment UUID, never its root key".to_owned(),
        );
    }
    Uuid::parse_str(value).map_err(|_| "expected a valid testing-environment UUID".to_owned())
}

/// Which container a base-level folder belongs in.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum RootTypeArg {
    /// The Public container, readable by every member.
    Public,
    /// The caller's own folder inside Private.
    Private,
    /// A tag's container.
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

/// What to answer an access request.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DecisionArg {
    /// Create the grant.
    Approve,
    /// Create nothing.
    Deny,
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

/// A member, as `carbon:cos:tos` or `silicon:atlas`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Principal(pub ActorRef);

impl FromStr for Principal {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, id) = value.trim().split_once(':').ok_or_else(|| {
            "a member is written kind:id, such as carbon:cos:tos or silicon:atlas".to_owned()
        })?;
        let actor_type = match kind.trim().to_ascii_lowercase().as_str() {
            "carbon" => ActorType::Carbon,
            "silicon" => ActorType::Silicon,
            other => return Err(format!("{other} is not carbon or silicon")),
        };
        if id.trim().is_empty() {
            return Err("a member identifier is required after the kind".to_owned());
        }
        Ok(Self(ActorRef {
            actor_type,
            id: id.trim().to_owned(),
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

/// An invitation attached to a new folder, as `carbon:cos:tos=read,write`.
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
            "an invitation is written principal=rights, such as carbon:cos:tos=read,write"
                .to_owned()
        })?;
        Ok(Self {
            principal: principal.parse::<Principal>()?.0,
            access: rights.parse::<Rights>()?.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Invitation, Principal, Rights, Target};
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
        let principal: Principal = "carbon:cos:tos".parse().unwrap();
        assert_eq!(principal.0.id, "cos:tos");
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
        let invitation: Invitation = "silicon:atlas=read,update".parse().unwrap();
        assert_eq!(invitation.principal.id, "atlas");
        assert_eq!(invitation.access.len(), 2);
        assert!("silicon:atlas".parse::<Invitation>().is_err());
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
            "tos>briefcase",
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
                "tos>notes",
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
                "notes",
                "--proof",
                "proof",
                "note.md",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "briefcase",
                "app",
                "upload",
                "--app-id",
                "tos>notes",
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
                "tos>notes",
                "--proof",
                "proof",
                "--proof-stdin",
                "note.md",
            ])
            .is_err()
        );
    }
}
