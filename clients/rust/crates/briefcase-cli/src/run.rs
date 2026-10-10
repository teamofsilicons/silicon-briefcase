//! Turning a parsed command into calls on the package.

use std::collections::HashSet;

use briefcase_client::{
    AccessRight, BucketConfiguration, Client, Destination, Entry, EntryPage, ExpiryChange,
    IdempotencyKey, ListEntries, NewFolder, NewGrant, PermissionQuery, Upload, guess_content_type,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

pub use crate::error::{CliError, Result};
use crate::{
    cli::{
        AllowCommand, AppCommand, BinCommand, Cli, Command, ConfigCommand, DriveArg, ExpiryArgs,
        FindArgs, GetArgs, GlobalArgs, KeepArgs, Lifetime, LoginCommand, LsArgs, MkdirArgs, MvArgs,
        PutArgs, RestoreArgs, RmArgs, SearchArgs, ShareArgs, SharedArgs, StorageCommand, Target,
        TargetArgs, UnshareArgs,
    },
    render::{Labels, Output, deadline, human_size},
    session::{self, Context},
    signin,
    state::{PendingMutation, StateDirectory},
};

mod delegated;

/// Runs one parsed invocation.
///
/// # Errors
///
/// Returns whatever stopped the command, carrying the exit code it deserves.
pub async fn run(cli: Cli) -> Result<()> {
    refuse_retired(&cli)?;
    let output = Output::new(cli.global.json);
    let global = &cli.global;
    match cli.command {
        Command::Login(args) => match args.command {
            Some(LoginCommand::Status { offline }) => {
                signin::login_status(global, offline, output).await
            }
            None => signin::login(global, &args, output).await,
        },
        Command::Logout => signin::logout(global, output).await,
        Command::Accounts | Command::Iam => {
            signin::accounts(global, output);
            Ok(())
        }
        Command::Status => signin::status(global, output).await,
        Command::Drives => drives(global, output).await,
        Command::Shared(args) => shared(global, &args, output).await,
        Command::Allow(command) => allow(global, &command, output).await,
        Command::Ls(args) => list(global, &args, output).await,
        Command::Find(args) => find(global, &args, output).await,
        Command::Search(args) => search(global, &args, output).await,
        Command::Stat(args) => stat(global, &args, output).await,
        Command::Mkdir(args) => mkdir(global, &args, output).await,
        Command::Put(args) => put(global, &args, output).await,
        Command::Get(args) => get(global, &args, output).await,
        Command::Cat(args) => cat(global, &args, output).await,
        Command::Archive(args) => {
            let mut context = session::connect(global).await?;
            let id = entry_id(&mut context, &args.target).await?;
            output.archive(&context.client.archive_contents(id, args.limit).await?);
            Ok(())
        }
        Command::Mv(args) => move_entry(global, &args, output).await,
        Command::Rm(args) => remove(global, &args, output).await,
        Command::Keep(args) => keep(global, &args, output).await,
        Command::Bin(command) => bin(global, &command, output).await,
        Command::Versions { target, cursor } => {
            versions(global, &target, cursor.as_deref(), output).await
        }
        Command::Restore(args) => restore_version(global, &args, output).await,
        Command::History(args) => {
            let mut context = session::connect(global).await?;
            let id = entry_id(&mut context, &args.target).await?;
            let events = context.client.activity(id).await?;
            output.history(&events, &labels(&context));
            Ok(())
        }
        Command::Logs { target, cursor } => {
            let mut context = session::connect(global).await?;
            let id = entry_id(&mut context, &target).await?;
            output.json(&context.client.logs(id, cursor.as_deref()).await?);
            Ok(())
        }
        Command::Link {
            target,
            enabled,
            expires_after,
        } => link(global, &target, enabled, expires_after, output).await,
        Command::Share(args) => share(global, &args, output).await,
        Command::Unshare(args) => unshare(global, &args, output).await,
        Command::Expiry(args) => change_expiring_share(global, &args, output).await,
        Command::Shares { target, cursor } => {
            let mut context = session::connect(global).await?;
            let id = entry_id(&mut context, &target).await?;
            output.invitations(&context.client.invitations(id, cursor.as_deref()).await?);
            Ok(())
        }
        Command::Access(args) => access(global, &args.targets, output).await,
        Command::Inbox(args) => {
            let context = session::connect(global).await?;
            let inbox = if args.read {
                context.client.mark_notifications_read().await?
            } else {
                context.client.notifications().await?
            };
            output.inbox(&inbox, &labels(&context));
            Ok(())
        }
        Command::Usage { drive } => usage(global, drive.as_ref(), output).await,
        Command::Storage(command) => storage(global, &command, output).await,
        Command::App(command) => application(global, &command, output).await,
        Command::Config(command) => configure(global, &command, output),
        Command::Report { message, pr } => report(global, message, pr, output).await,
        Command::Daemon(command) => crate::daemon::command(&command, output).await,
        Command::Docs { topic } => crate::manual::show(topic.as_deref(), output),
        Command::Version => version(global, output).await,
        Command::Env { .. } => Err(CliError::retired(
            "`briefcase env`",
            "Testing environments no longer exist. Test against a local Briefcase and a local Silicon Accounts: \
             `briefcase --url http://127.0.0.1:4101/api/v1/ --accounts-url http://localhost:9590 login --save-as local`.",
        )),
        Command::System { .. } => Err(CliError::retired(
            "`briefcase system`",
            "Silicon Apps installs and updates briefcase (`silicon-apps install briefcase`); there is nothing to update by hand.",
        )),
    }
}

/// Commands that never contact Briefcase or Silicon Accounts.
const fn is_local(command: &Command) -> bool {
    matches!(
        command,
        Command::Accounts
            | Command::Iam
            | Command::Docs { .. }
            | Command::Config(_)
            | Command::Daemon(_)
            | Command::Env { .. }
            | Command::System { .. }
    )
}

/// Refuses options and settings from before Silicon Accounts, saying what replaced them.
fn refuse_retired(cli: &Cli) -> Result<()> {
    if cli.global.org.is_some() {
        return Err(CliError::retired(
            "`--org`",
            "Every Carbon and Silicon has its own drive now. Name another drive with @ and its owner's id, \
             such as `briefcase ls @c:ada/plans`.",
        ));
    }
    if cli.global.test.is_some() || cli.global.app_secret.is_some() {
        return Err(CliError::retired(
            "`--test` / `--app-secret`",
            "Testing environments no longer exist. Test against a local Briefcase and a local Silicon Accounts \
             with their own profile (`--url`, `--accounts-url`, `login --save-as local`).",
        ));
    }
    let testing = std::env::var("BRIEFCASE_TEST").is_ok_and(|value| !value.trim().is_empty());
    let offline_status = matches!(
        &cli.command,
        Command::Login(args) if matches!(args.command, Some(LoginCommand::Status { offline: true }))
    );
    if testing && !is_local(&cli.command) && !offline_status {
        return Err(CliError::refused(
            "testing_environments_retired",
            "BRIEFCASE_TEST is set, but testing environments no longer exist; refusing to run against a real deployment in its place.",
            Some(
                "Unset BRIEFCASE_TEST. Test against a local Briefcase with its own profile (`--url`, `--accounts-url`)."
                    .to_owned(),
            ),
            2,
        ));
    }
    Ok(())
}

// ---- targets ----------------------------------------------------------------

/// Ids by uuid for what the CLI already knows: the signed-in account.
fn labels(context: &Context) -> Labels {
    let mut labels = Labels::default();
    if let Some(account) = context.account() {
        labels.learn(&account.uuid, account.display_id());
    }
    labels
}

/// Learns that a drive named by id holds `entries`.
fn learn_drive(labels: &mut Labels, target: Option<&Target>, entries: &[Entry]) {
    if let Some(drive) = target.and_then(Target::drive)
        && drive.contains(':')
        && let Some(entry) = entries.first()
    {
        labels.learn(&entry.drive, drive);
    }
}

/// Resolves a target to the entry it names.
async fn resolve_entry(context: &mut Context, target: &Target) -> Result<Entry> {
    match target {
        Target::Id(id) => Ok(context.client.entry(*id).await?),
        Target::Path { drive, path } => {
            let owner = match drive {
                Some(drive) => drive.clone(),
                None => context.own_uuid().await?,
            };
            Ok(context.client.entry_at(&owner, path).await?)
        }
        Target::Legacy { organization, path } => Ok(context
            .client
            .entry_at_legacy_address(organization, path)
            .await?),
    }
}

/// Resolves a target to an entry identifier, looking a path up when needed.
async fn entry_id(context: &mut Context, target: &Target) -> Result<Uuid> {
    match target {
        Target::Id(id) => Ok(*id),
        _ => Ok(resolve_entry(context, target).await?.id),
    }
}

/// A folder target in the shape the package takes.
async fn destination(context: &mut Context, target: &Target) -> Result<Destination> {
    Ok(match target {
        Target::Id(id) => Destination::Id(*id),
        Target::Path { drive, path } => Destination::Path {
            drive: drive.clone(),
            path: path.clone(),
        },
        Target::Legacy { .. } => Destination::Id(resolve_entry(context, target).await?.id),
    })
}

/// Splits `a/b/c` into the parent path `a/b` and the name `c`.
fn split_path(path: &str) -> (Option<String>, String) {
    let trimmed = path.trim_matches('/');
    trimmed.rsplit_once('/').map_or_else(
        || (None, trimmed.to_owned()),
        |(parent, name)| (Some(parent.to_owned()), name.to_owned()),
    )
}

// ---- durable retries ----------------------------------------------------------

fn request_fingerprint(intent: &impl Serialize) -> Result<String> {
    let body = serde_json::to_vec(intent)
        .map_err(|error| CliError::usage(format!("could not identify retry intent: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(body)))
}

/// Reads the stable resources selected by an earlier uncertain attempt.
fn durable_pending(scope: &str, fingerprint: &str) -> Result<Option<PendingMutation>> {
    let state = StateDirectory::locate()?;
    let _state_lock = state.lock_credentials()?;
    let credentials = state.credentials()?;
    Ok(credentials
        .pending_mutation(scope)
        .filter(|pending| pending.request_fingerprint == fingerprint)
        .cloned())
}

fn pending_resource_id(scope: &str, fingerprint: &str) -> Result<Option<Uuid>> {
    Ok(durable_pending(scope, fingerprint)?.and_then(|pending| pending.resource_id))
}

/// Persists a content mutation's retry identity before its request starts, so
/// rerunning the exact command after an uncertain result recovers the
/// original answer instead of applying it twice.
fn prepare_durable_mutation(
    scope: &str,
    fingerprint: &str,
    resource_id: Option<Uuid>,
    destination_id: Option<Uuid>,
) -> Result<PendingMutation> {
    let state = StateDirectory::locate()?;
    let _state_lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    let mut pending = credentials.prepare_mutation(scope, fingerprint);
    if pending.resource_id.is_none()
        && let Some(resource_id) = resource_id
    {
        pending.resource_id = Some(resource_id);
        credentials.set_pending_mutation(scope.to_owned(), pending.clone());
    }
    if pending.destination_id.is_none()
        && let Some(destination_id) = destination_id
    {
        pending.destination_id = Some(destination_id);
        credentials.set_pending_mutation(scope.to_owned(), pending.clone());
    }
    state.save_credentials(&credentials)?;
    Ok(pending)
}

/// Clears only the mutation identity whose response was observed locally.
fn finish_durable_mutation(scope: &str, pending: &PendingMutation) -> Result<()> {
    let state = StateDirectory::locate()?;
    let _state_lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    credentials.clear_pending_mutation(scope, &pending.idempotency_key);
    state.save_credentials(&credentials)?;
    Ok(())
}

/// The folder a destination names, by identifier, reusing the one an earlier
/// uncertain attempt resolved.
async fn stable_destination_id(
    context: &mut Context,
    destination: &Destination,
    recovered: Option<&PendingMutation>,
) -> Result<Uuid> {
    if let Some(id) = recovered.and_then(|pending| pending.destination_id) {
        return Ok(id);
    }
    match destination {
        Destination::Id(id) => Ok(*id),
        Destination::Path { drive, path } => {
            let owner = match drive {
                Some(drive) => drive.clone(),
                None => context.own_uuid().await?,
            };
            Ok(context.client.entry_at(&owner, path).await?.id)
        }
    }
}

async fn stable_upload_destination_id(
    context: &mut Context,
    upload: &Upload,
    recovered: Option<&PendingMutation>,
) -> Result<Uuid> {
    match stable_destination_id(context, &upload.destination, recovered).await {
        Err(CliError::Client(parent_error)) if parent_error.is_not_found() => {
            let Destination::Path { drive, path } = &upload.destination else {
                return Err(parent_error.into());
            };
            let file_name = upload.file_name.trim();
            if file_name.is_empty()
                || file_name.contains(['/', '\0'])
                || matches!(file_name, "." | "..")
            {
                return Err(parent_error.into());
            }
            // A shared file may be readable and updatable while its folder is
            // not; the file names the same folder without granting access to
            // new siblings. Briefcase still decides replacement versus creation.
            let owner = match drive {
                Some(drive) => drive.clone(),
                None => context.own_uuid().await?,
            };
            let file_path = if path.is_empty() {
                file_name.to_owned()
            } else {
                format!("{path}/{file_name}")
            };
            let existing = match context.client.entry_at(&owner, &file_path).await {
                Ok(existing) if !existing.is_folder() => existing,
                Ok(_) => return Err(parent_error.into()),
                Err(error) if error.is_not_found() => return Err(parent_error.into()),
                Err(error) => return Err(error.into()),
            };
            existing.parent_id.ok_or_else(|| parent_error.into())
        }
        result => result,
    }
}

async fn file_identity(path: &std::path::Path) -> Result<(String, String)> {
    let canonical = tokio::fs::canonicalize(path)
        .await
        .map_err(|source| CliError::Io {
            path: path.display().to_string(),
            source,
        })?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| CliError::Io {
            path: path.display().to_string(),
            source,
        })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| CliError::Io {
                path: path.display().to_string(),
                source,
            })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((
        canonical.display().to_string(),
        format!("{:x}", digest.finalize()),
    ))
}

// ---- drives -------------------------------------------------------------------

async fn drives(global: &GlobalArgs, output: Output) -> Result<()> {
    let context = session::connect(global).await?;
    output.drives(&context.client.drives().await?);
    Ok(())
}

async fn shared(global: &GlobalArgs, args: &SharedArgs, output: Output) -> Result<()> {
    let context = session::connect(global).await?;
    let mut page = context
        .client
        .shared(args.cursor.as_deref(), args.limit)
        .await?;
    if args.all {
        let mut seen = HashSet::new();
        if let Some(cursor) = &args.cursor {
            seen.insert(cursor.clone());
        }
        while let Some(cursor) = unique_next_cursor(&mut seen, page.next_cursor.take())? {
            let next = context.client.shared(Some(&cursor), args.limit).await?;
            page.items.extend(next.items);
            page.next_cursor = next.next_cursor;
        }
    }
    output.shared(&page);
    Ok(())
}

/// The Silicon an allow-list command is for: `--for`, or the signed-in Silicon.
async fn allow_target(context: &mut Context, silicon: Option<&DriveArg>) -> Result<String> {
    if let Some(silicon) = silicon {
        return Ok(silicon.0.clone());
    }
    context.own_uuid().await?;
    match context.account() {
        Some(account) if account.kind == briefcase_client::AccountKind::Silicon => {
            Ok(account.uuid.clone())
        }
        _ => Err(CliError::usage(
            "name the Silicon with --for <si:id>: a Carbon manages the allow list of a Silicon it looks after",
        )),
    }
}

async fn allow(global: &GlobalArgs, command: &AllowCommand, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    match command {
        AllowCommand::List { silicon } => {
            let silicon = allow_target(&mut context, silicon.as_ref()).await?;
            output.allowed(&context.client.allowed_accounts(&silicon).await?);
        }
        AllowCommand::Add { account, silicon } => {
            let silicon = allow_target(&mut context, silicon.as_ref()).await?;
            context
                .client
                .allow_account(&silicon, &account.0.id)
                .await?;
            let allowed = context.client.allowed_accounts(&silicon).await?;
            if output.is_json() {
                output.json(&allowed);
            } else {
                output.note(&format!(
                    "{} may now share with {}.",
                    account.0.id,
                    allowed.silicon.display_id()
                ));
            }
        }
        AllowCommand::Remove { account, silicon } => {
            let silicon = allow_target(&mut context, silicon.as_ref()).await?;
            context
                .client
                .disallow_account(&silicon, &account.0.id)
                .await?;
            let allowed = context.client.allowed_accounts(&silicon).await?;
            if output.is_json() {
                output.json(&allowed);
            } else {
                output.note(&format!(
                    "{} may no longer share with {}; shares it already made stay until they are revoked.",
                    account.0.id,
                    allowed.silicon.display_id()
                ));
            }
        }
    }
    Ok(())
}

// ---- browsing -----------------------------------------------------------------

async fn list(global: &GlobalArgs, args: &LsArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let parent = match &args.target {
        Some(target) => Some(destination(&mut context, target).await?),
        None => None,
    };
    let query = ListEntries {
        parent,
        cursor: args.cursor.clone(),
        limit: args.limit,
        ..ListEntries::default()
    };
    let page = if args.all {
        EntryPage {
            items: list_every_entry(&context.client, query).await?,
            next_cursor: None,
        }
    } else {
        context.client.list_entries(&query).await?
    };
    let mut labels = labels(&context);
    learn_drive(&mut labels, args.target.as_ref(), &page.items);
    output.entry_page(&page, args.long, &labels);
    Ok(())
}

async fn find(global: &GlobalArgs, args: &FindArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let parent = match &args.in_folder {
        Some(target) => Some(destination(&mut context, target).await?),
        None => None,
    };
    let query = ListEntries {
        parent,
        cursor: args.cursor.clone(),
        limit: args.limit,
        ..ListEntries::matching(&args.filter)
    };
    let page = if args.all {
        EntryPage {
            items: list_every_entry(&context.client, query).await?,
            next_cursor: None,
        }
    } else {
        context.client.list_entries(&query).await?
    };
    let mut labels = labels(&context);
    learn_drive(&mut labels, args.in_folder.as_ref(), &page.items);
    output.entry_page(&page, true, &labels);
    Ok(())
}

async fn list_every_entry(client: &Client, mut query: ListEntries) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut seen_cursors = HashSet::new();
    if let Some(cursor) = &query.cursor {
        seen_cursors.insert(cursor.clone());
    }
    loop {
        let page = client.list_entries(&query).await?;
        entries.extend(page.items);
        let Some(cursor) = unique_next_cursor(&mut seen_cursors, page.next_cursor)? else {
            return Ok(entries);
        };
        query.cursor = Some(cursor);
    }
}

fn unique_next_cursor(
    seen_cursors: &mut HashSet<String>,
    next_cursor: Option<String>,
) -> Result<Option<String>> {
    let Some(cursor) = next_cursor else {
        return Ok(None);
    };
    if !seen_cursors.insert(cursor.clone()) {
        return Err(briefcase_client::Error::Protocol(
            "Briefcase repeated a pagination cursor while --all was following pages".to_owned(),
        )
        .into());
    }
    Ok(Some(cursor))
}

async fn search(global: &GlobalArgs, args: &SearchArgs, output: Output) -> Result<()> {
    let context = session::connect(global).await?;
    let results = context.client.search(&args.query, args.limit).await?;
    output.search(&results);
    Ok(())
}

async fn stat(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let entry = resolve_entry(&mut context, &args.target).await?;
    let mut labels = labels(&context);
    learn_drive(
        &mut labels,
        Some(&args.target),
        std::slice::from_ref(&entry),
    );
    output.entry(&entry, &labels);
    Ok(())
}

// ---- creating and moving bytes ----------------------------------------------------

async fn mkdir(global: &GlobalArgs, args: &MkdirArgs, output: Output) -> Result<()> {
    let Target::Path { drive, path } = &args.path else {
        return Err(CliError::usage(
            "mkdir takes the new folder's path, such as notes/2026 or @si:scout/reports",
        ));
    };
    let (parent, name) = split_path(path);
    if name.is_empty() {
        return Err(CliError::usage("a folder name is required"));
    }
    let mut context = session::connect(global).await?;
    let mut folder = NewFolder::in_folder(
        name,
        Destination::Path {
            drive: drive.clone(),
            path: parent.clone().unwrap_or_default(),
        },
    );
    folder.invitees = args
        .invites
        .iter()
        .map(|invitation| {
            NewGrant::new(invitation.principal.clone(), invitation.access.clone()).inheriting()
        })
        .collect();

    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "create-folder",
        "profile": &context.profile,
        "url": context.client.config().api_base().as_str(),
        "drive": drive,
        "name": &folder.name,
        "parent": &parent,
        "invitees": &folder.invitees,
    }))?;
    let address = request_fingerprint(&serde_json::json!({"drive": drive, "path": path}))?;
    let scope = format!("entry:mkdir:{}:{address}", context.profile);
    let recovered = durable_pending(&scope, &fingerprint)?;
    let parent_destination = folder.parent.clone().unwrap_or_else(Destination::top);
    let destination_id =
        stable_destination_id(&mut context, &parent_destination, recovered.as_ref()).await?;
    let pending = prepare_durable_mutation(&scope, &fingerprint, None, Some(destination_id))?;
    folder.parent = Some(Destination::Id(pending.destination_id.ok_or_else(
        || CliError::usage("the pending folder creation has no destination identifier"),
    )?));
    folder.idempotency_key = Some(IdempotencyKey::new(pending.idempotency_key.clone())?);
    let created = context.client.create_folder(&folder).await?;
    finish_durable_mutation(&scope, &pending)?;
    if output.is_json() {
        output.json(&created);
    } else {
        output.note(&format!(
            "created {} ({})",
            created.path, created.permanent_url
        ));
    }
    Ok(())
}

async fn put(global: &GlobalArgs, args: &PutArgs, output: Output) -> Result<()> {
    if args.name.is_some() && args.sources.len() > 1 {
        return Err(CliError::usage("--name works with a single file"));
    }
    let mut context = session::connect(global).await?;
    let target_destination = destination(&mut context, &args.destination).await?;
    let mut stored = Vec::with_capacity(args.sources.len());
    for source in &args.sources {
        let mut upload = Upload::file(target_destination.clone(), source.clone())?;
        if let Some(name) = &args.name {
            upload = upload.named(name.clone());
        }
        if let Some(Lifetime(minutes)) = args.self_destruct {
            upload = upload.self_destructing(minutes);
        }
        let content_type = args
            .content_type
            .clone()
            .unwrap_or_else(|| guess_content_type(&upload.file_name).to_owned());
        upload = upload.with_content_type(content_type);

        let (canonical_source, content_sha256) = file_identity(source).await?;
        let destination = args.destination.to_string();
        let address = request_fingerprint(&serde_json::json!({
            "source": &canonical_source,
            "destination": &destination,
            "file_name": &upload.file_name,
        }))?;
        let scope = format!("entry:put:{}:{address}", context.profile);
        let mut intent = serde_json::json!({
            "operation": "upload-file",
            "profile": &context.profile,
            "url": context.client.config().api_base().as_str(),
            "source": canonical_source,
            "destination": destination,
            "file_name": &upload.file_name,
            "content_type": &upload.content_type,
            "content_sha256": content_sha256,
        });
        if let Some(minutes) = upload.self_destruct_minutes {
            intent["self_destruct_minutes"] = minutes.into();
        }
        let fingerprint = request_fingerprint(&intent)?;
        let recovered = durable_pending(&scope, &fingerprint)?;
        let destination_id =
            stable_upload_destination_id(&mut context, &upload, recovered.as_ref()).await?;
        let pending = prepare_durable_mutation(&scope, &fingerprint, None, Some(destination_id))?;
        upload.destination =
            Destination::Id(pending.destination_id.ok_or_else(|| {
                CliError::usage("the pending upload has no destination identifier")
            })?);
        upload.idempotency_key = Some(IdempotencyKey::new(pending.idempotency_key.clone())?);
        let entry = context.client.upload(&upload).await.map_err(|error| {
            CliError::explained(error, |error| {
                (error.code() == Some("self_destruct_requires_new_file")).then(|| {
                    format!(
                        "--self-destruct applies only to a new file, and `{}` already exists in that folder; upload under another name with --name, or drop --self-destruct to publish a new version of the existing file",
                        upload.file_name
                    )
                })
            })
        })?;
        finish_durable_mutation(&scope, &pending)?;
        if !output.is_json() {
            println!(
                "{} → {} ({}){}",
                source.display(),
                entry.path,
                entry.size.map_or_else(|| "-".to_owned(), human_size),
                entry
                    .self_destruct_at
                    .map_or_else(String::new, |moment| format!(
                        ", self-destructs {}",
                        deadline(moment, time::OffsetDateTime::now_utc())
                    ))
            );
        }
        stored.push(entry);
    }
    if output.is_json() {
        output.json(&stored);
    }
    Ok(())
}

async fn get(global: &GlobalArgs, args: &GetArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let entry = resolve_entry(&mut context, &args.target).await?;
    let destination = args.output.clone().unwrap_or_else(|| {
        std::path::PathBuf::from(if entry.is_folder() {
            format!(
                "{}.tar.zst",
                if entry.name.is_empty() {
                    "drive"
                } else {
                    &entry.name
                }
            )
        } else {
            entry.name.clone()
        })
    });
    let written = context
        .client
        .download_to_file(entry.id, &destination)
        .await?;
    if output.is_json() {
        output.json(&serde_json::json!({
            "entry_id": entry.id,
            "path": entry.path,
            "written_to": destination.display().to_string(),
            "bytes": written,
        }));
    } else {
        println!(
            "{} → {} ({})",
            entry.path,
            destination.display(),
            human_size(written)
        );
    }
    Ok(())
}

async fn cat(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let id = entry_id(&mut context, &args.target).await?;
    let mut stream = context.client.read_content(id, None).await?;
    while let Some(chunk) = stream.chunk().await? {
        output.bytes(&chunk).map_err(|source| CliError::Io {
            path: "standard output".to_owned(),
            source,
        })?;
    }
    Ok(())
}

async fn move_entry(global: &GlobalArgs, args: &MvArgs, output: Output) -> Result<()> {
    let Target::Path {
        drive: destination_drive,
        path: destination_path,
    } = &args.destination
    else {
        return Err(CliError::usage(
            "the destination is the entry's new path, such as notes/renamed.md or @si:scout/archive/old.md",
        ));
    };
    let (parent_path, name) = split_path(destination_path);
    if name.is_empty() {
        return Err(CliError::usage("a destination name is required"));
    }
    let mut context = session::connect(global).await?;
    let address = request_fingerprint(&serde_json::json!({
        "target": args.target.to_string(),
        "destination": args.destination.to_string(),
    }))?;
    let scope = format!("entry:move:{}:{address}", context.profile);
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "update-entry",
        "profile": &context.profile,
        "url": context.client.config().api_base().as_str(),
        "target": args.target.to_string(),
        "destination": args.destination.to_string(),
    }))?;
    let recovered = durable_pending(&scope, &fingerprint)?;
    let (resource_id, original_path, original_parent_id) =
        if let Some(id) = recovered.as_ref().and_then(|pending| pending.resource_id) {
            (id, args.target.to_string(), None)
        } else {
            let entry = resolve_entry(&mut context, &args.target).await?;
            (entry.id, entry.path, entry.parent_id)
        };
    // A rename in place already knows its folder (reading a shared file does not
    // imply its folder is readable). That holds only when the destination names
    // the same drive the same way as the target.
    let same_drive = args.target.drive() == destination_drive.as_deref()
        && !matches!(args.target, Target::Legacy { .. });
    let known_destination_id = recovered
        .as_ref()
        .and_then(|pending| pending.destination_id)
        .or_else(|| {
            original_parent_id.filter(|_| same_drive && split_path(&original_path).0 == parent_path)
        });
    let destination_id = if let Some(id) = known_destination_id {
        id
    } else {
        let parent = Destination::Path {
            drive: destination_drive.clone(),
            path: parent_path.clone().unwrap_or_default(),
        };
        stable_destination_id(&mut context, &parent, recovered.as_ref()).await?
    };
    let pending = prepare_durable_mutation(
        &scope,
        &fingerprint,
        Some(resource_id),
        Some(destination_id),
    )?;
    let entry_id = pending.resource_id.unwrap_or(resource_id);
    let mut update = briefcase_client::EntryUpdate::rename(name);
    update.parent_id = pending.destination_id;
    let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
    let moved = context
        .client
        .update_entry_with_key(entry_id, &update, &key)
        .await?;
    finish_durable_mutation(&scope, &pending)?;
    if output.is_json() {
        output.json(&moved);
    } else {
        output.note(&format!("{original_path} → {}", moved.path));
    }
    Ok(())
}

async fn remove(global: &GlobalArgs, args: &RmArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    for target in &args.targets {
        let entry = resolve_entry(&mut context, target).await?;
        context.client.delete_entry(entry.id).await?;
        if entry.self_destruct_at.is_some() {
            output.note(&format!(
                "{} deleted permanently; a self-destructing file never enters the bin ({})",
                entry.path, entry.id
            ));
        } else {
            output.note(&format!(
                "{} moved to the bin, recoverable for 45 days ({})",
                entry.path, entry.id
            ));
        }
    }
    Ok(())
}

async fn keep(global: &GlobalArgs, args: &KeepArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let mut kept = Vec::with_capacity(args.targets.len());
    for target in &args.targets {
        let entry = resolve_entry(&mut context, target).await?;
        // A lost response needs no stored retry identity: stopping a stopped
        // timer is refused as `not_self_destructing`, never applied twice.
        context.client.make_permanent(entry.id).await.map_err(|error| {
            CliError::explained(error, |error| {
                if error.is_forbidden() {
                    Some(format!(
                        "only the creator of {}, its drive's owner, and the custodian of either can keep a self-destructing file",
                        entry.path
                    ))
                } else if error.code() == Some("not_self_destructing") {
                    Some(format!(
                        "{} has no running self-destruct timer: it is already permanent, or it was never set to self-destruct",
                        entry.path
                    ))
                } else {
                    None
                }
            })
        })?;
        output.note(&format!(
            "{} kept; it will no longer self-destruct ({})",
            entry.path, entry.id
        ));
        kept.push(serde_json::json!({"entry_id": entry.id, "path": entry.path}));
    }
    if output.is_json() {
        output.json(&kept);
    }
    Ok(())
}

async fn bin(global: &GlobalArgs, command: &BinCommand, output: Output) -> Result<()> {
    let context = session::connect(global).await?;
    match command {
        BinCommand::List {
            drive,
            limit,
            cursor,
            all,
        } => {
            let drive = drive.as_ref().map(|drive| drive.0.as_str());
            let mut page = context.client.bin(drive, cursor.as_deref(), *limit).await?;
            if *all {
                let mut seen = HashSet::new();
                if let Some(cursor) = cursor {
                    seen.insert(cursor.clone());
                }
                while let Some(next) = unique_next_cursor(&mut seen, page.next_cursor.take())? {
                    let more = context.client.bin(drive, Some(&next), *limit).await?;
                    page.items.extend(more.items);
                    page.next_cursor = more.next_cursor;
                }
            }
            output.entry_page(&page, true, &labels(&context));
        }
        BinCommand::Restore { entry_id } => {
            let scope = format!("entry:restore-bin:{}:{entry_id}", context.profile);
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "restore-bin",
                "profile": &context.profile,
                "url": context.client.config().api_base().as_str(),
                "entry_id": entry_id,
            }))?;
            let pending = prepare_durable_mutation(&scope, &fingerprint, Some(*entry_id), None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let entry = context
                .client
                .restore_from_bin_with_key(*entry_id, &key)
                .await?;
            finish_durable_mutation(&scope, &pending)?;
            if output.is_json() {
                output.json(&entry);
            } else {
                output.note(&format!("restored to {}", entry.path));
            }
        }
    }
    Ok(())
}

async fn versions(
    global: &GlobalArgs,
    target: &Target,
    cursor: Option<&str>,
    output: Output,
) -> Result<()> {
    let mut context = session::connect(global).await?;
    let id = entry_id(&mut context, target).await?;
    let versions = context.client.versions_page(id, cursor).await?;
    if output.is_json() {
        output.json(&versions);
    } else {
        output.versions(&versions.items, &labels(&context));
        if let Some(cursor) = versions.next_cursor {
            output.note(&format!("More versions available: {cursor}"));
        }
    }
    Ok(())
}

async fn restore_version(global: &GlobalArgs, args: &RestoreArgs, output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let address = request_fingerprint(&serde_json::json!({
        "target": args.target.to_string(),
        "version_id": args.version_id,
    }))?;
    let scope = format!("entry:restore-version:{}:{address}", context.profile);
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "restore-version",
        "profile": &context.profile,
        "url": context.client.config().api_base().as_str(),
        "target": args.target.to_string(),
        "version_id": args.version_id,
    }))?;
    let id = match pending_resource_id(&scope, &fingerprint)? {
        Some(id) => id,
        None => entry_id(&mut context, &args.target).await?,
    };
    let pending = prepare_durable_mutation(&scope, &fingerprint, Some(id), None)?;
    let id = pending.resource_id.unwrap_or(id);
    let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
    let entry = context
        .client
        .restore_version_with_key(id, args.version_id, &key)
        .await?;
    finish_durable_mutation(&scope, &pending)?;
    if output.is_json() {
        output.json(&entry);
    } else {
        output.note(&format!(
            "{} now serves version {}",
            entry.path, args.version_id
        ));
    }
    Ok(())
}

// ---- sharing ------------------------------------------------------------------

async fn share(global: &GlobalArgs, args: &ShareArgs, output: Output) -> Result<()> {
    if args.expires_after.is_some()
        && args
            .access
            .0
            .iter()
            .any(|right| *right != AccessRight::Read)
    {
        return Err(CliError::usage(format!(
            "an expiring share is read-only, but --access asked for {}; omit --access (read is the default) or pass --access read",
            args.access
                .0
                .iter()
                .map(|right| right.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )));
    }
    let principal = match args.principal.0.actor_type {
        briefcase_client::ActorType::Silicon => {
            briefcase_client::Recipient::Silicon(args.principal.0.id.clone())
        }
        _ => briefcase_client::Recipient::Carbon(args.principal.0.id.clone()),
    };
    let grant = briefcase_client::Invite {
        principal,
        access: args.access.0.clone(),
        inherit: args.inherit,
        expires_in_minutes: args.expires_after.map(|Lifetime(minutes)| minutes),
    };
    let (context, scope, pending, id) =
        sharing_mutation(global, &args.target, "invite", &grant).await?;
    let invitation = context
        .client
        .invite(
            id,
            &grant,
            &IdempotencyKey::new(pending.idempotency_key.clone())?,
        )
        .await?;
    finish_durable_mutation(&scope, &pending)?;
    output.invitation_to(&invitation, &args.principal.0.id);
    Ok(())
}

async fn unshare(global: &GlobalArgs, args: &UnshareArgs, output: Output) -> Result<()> {
    let (context, scope, pending, id) =
        sharing_mutation(global, &args.target, "revoke-invitation", &args.grant_id).await?;
    context
        .client
        .revoke_invitation(
            id,
            args.grant_id,
            &IdempotencyKey::new(pending.idempotency_key.clone())?,
        )
        .await?;
    finish_durable_mutation(&scope, &pending)?;
    output.note("Share revoked. Access the account holds by another route still applies.");
    Ok(())
}

async fn change_expiring_share(
    global: &GlobalArgs,
    args: &ExpiryArgs,
    output: Output,
) -> Result<()> {
    let change = match (args.expires_in, args.permanent) {
        (Some(Lifetime(minutes)), false) => ExpiryChange::ExpireIn(minutes),
        (None, true) => ExpiryChange::Permanent,
        _ => {
            return Err(CliError::usage(
                "pass exactly one of --expires-in <DURATION> or --permanent",
            ));
        }
    };
    let (context, scope, pending, id) = sharing_mutation(
        global,
        &args.target,
        "change-expiring-share",
        &serde_json::json!({"grant_id": args.grant_id, "change": change}),
    )
    .await?;
    let invitation = context
        .client
        .change_expiring_share(
            id,
            args.grant_id,
            change,
            &IdempotencyKey::new(pending.idempotency_key.clone())?,
        )
        .await
        .map_err(|error| {
            CliError::explained(error, |error| {
                if error.is_not_found() {
                    Some(format!(
                        "share {} is not live on {}: an expiring share that has ended is gone for good. List live shares with `briefcase shares {}`, or share again with `briefcase share {} <account> --expires-after <DURATION>`",
                        args.grant_id, args.target, args.target, args.target
                    ))
                } else if error.code() == Some("not_an_expiring_share") {
                    Some(format!(
                        "share {} is permanent, so it has no timer to change; revoke it with `briefcase unshare {} {}`",
                        args.grant_id, args.target, args.grant_id
                    ))
                } else {
                    None
                }
            })
        })?;
    finish_durable_mutation(&scope, &pending)?;
    output.invitation(&invitation);
    if invitation.expires_at.is_none() {
        output.aside(&format!(
            "share {} is now permanent read access",
            invitation.id
        ));
    }
    Ok(())
}

async fn link(
    global: &GlobalArgs,
    target: &Target,
    enabled: Option<bool>,
    expires_after: Option<Lifetime>,
    output: Output,
) -> Result<()> {
    let value = if let Some(Lifetime(minutes)) = expires_after {
        let (context, scope, pending, id) = sharing_mutation(
            global,
            target,
            "link-access",
            &serde_json::json!({"enabled": true, "expires_in_minutes": minutes}),
        )
        .await?;
        let value = context
            .client
            .set_expiring_link_access(
                id,
                minutes,
                &IdempotencyKey::new(pending.idempotency_key.clone())?,
            )
            .await
            .map_err(|error| {
                CliError::explained(error, |error| {
                    (error.code() == Some("link_already_permanent")).then(|| {
                        format!(
                            "{target} already has a permanent link; turn it off with `briefcase link {target} --enabled false`, then run --expires-after again"
                        )
                    })
                })
            })?;
        finish_durable_mutation(&scope, &pending)?;
        value
    } else if let Some(enabled) = enabled {
        let (context, scope, pending, id) =
            sharing_mutation(global, target, "link-access", &enabled).await?;
        let value = context
            .client
            .set_link_access(
                id,
                enabled,
                &IdempotencyKey::new(pending.idempotency_key.clone())?,
            )
            .await?;
        finish_durable_mutation(&scope, &pending)?;
        value
    } else {
        let mut context = session::connect(global).await?;
        let id = entry_id(&mut context, target).await?;
        context.client.link_access(id).await?
    };
    output.link(&value);
    Ok(())
}

/// Binds a retry to its original entry ID even if the supplied path later moves.
async fn sharing_mutation<T: Serialize>(
    global: &GlobalArgs,
    target: &Target,
    operation: &str,
    body: &T,
) -> Result<(Context, String, PendingMutation, Uuid)> {
    let mut context = session::connect(global).await?;
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": operation,
        "target": target.to_string(),
        "body": body,
        "profile": &context.profile,
        "url": context.client.config().api_base().as_str(),
    }))?;
    let scope = format!("entry:{operation}:{}:{fingerprint}", context.profile);
    let id = match pending_resource_id(&scope, &fingerprint)? {
        Some(id) => id,
        None => entry_id(&mut context, target).await?,
    };
    let pending = prepare_durable_mutation(&scope, &fingerprint, Some(id), None)?;
    Ok((context, scope, pending, id))
}

async fn access(global: &GlobalArgs, targets: &[Target], output: Output) -> Result<()> {
    let mut context = session::connect(global).await?;
    let mut query = PermissionQuery::default();
    let mut drive: Option<Option<String>> = None;
    for target in targets {
        match target {
            Target::Id(id) => query.entry_ids.push(*id),
            Target::Path { drive: named, path } => {
                if drive.as_ref().is_some_and(|drive| drive != named) {
                    return Err(CliError::usage(
                        "`briefcase access` reports on paths in one drive at a time; run it once per drive (IDs may be mixed in freely)",
                    ));
                }
                drive = Some(named.clone());
                query.paths.push(path.clone());
            }
            Target::Legacy { .. } => query
                .entry_ids
                .push(resolve_entry(&mut context, target).await?.id),
        }
    }
    query.drive = drive.flatten();
    let inspection = context.client.effective_access(&query).await?;
    output.inspection(&inspection);
    Ok(())
}

async fn usage(global: &GlobalArgs, drive: Option<&DriveArg>, output: Output) -> Result<()> {
    let context = session::connect(global).await?;
    let drive = drive.map(|drive| drive.0.as_str());
    let usage = context.client.usage(drive).await?;
    let name = drive.map_or_else(
        || {
            context.account().map_or_else(
                || "yours".to_owned(),
                |account| format!("@{} (yours)", account.display_id()),
            )
        },
        |drive| format!("@{drive}"),
    );
    output.usage(&usage, &name);
    Ok(())
}

async fn storage(global: &GlobalArgs, command: &StorageCommand, output: Output) -> Result<()> {
    let StorageCommand::Configure(args) = command;
    let configuration = BucketConfiguration {
        drive: args.drive.as_ref().map(|drive| drive.0.clone()),
        bucket_name: args.bucket.clone(),
        region: args.region.clone(),
        role_arn: args.role_arn.clone(),
        prefix: args.prefix.clone(),
        aws_account_id: args.account.clone(),
        encryption_mode: args.encryption.into(),
        kms_key_arn: args.kms_key_arn.clone(),
    };
    let operation = args.operation_id.unwrap_or_else(Uuid::new_v4);
    if operation.is_nil() {
        return Err(CliError::usage("--operation-id must be a non-nil UUID"));
    }
    let context = session::connect(global).await?;
    eprintln!(
        "Storage operation ID: {operation}. Reuse --operation-id {operation} with the same configuration after a lost response."
    );
    let status = context
        .client
        .configure_storage_with_key(&configuration, &IdempotencyKey::new(operation.to_string())?)
        .await?;
    if output.is_json() {
        output.json(&status);
    } else {
        match status.status {
            briefcase_client::BucketConfigurationState::Configured => {
                println!(
                    "configured; new files in the drive now go to {}",
                    args.bucket
                );
            }
            briefcase_client::BucketConfigurationState::Failed => {
                println!(
                    "probe failed ({}); the previous configuration is still in use",
                    status
                        .failure_reason
                        .as_deref()
                        .unwrap_or("no reason given")
                );
            }
        }
    }
    if status.status == briefcase_client::BucketConfigurationState::Failed {
        return Err(CliError::StorageValidationFailed);
    }
    Ok(())
}

async fn application(global: &GlobalArgs, command: &AppCommand, output: Output) -> Result<()> {
    match command {
        AppCommand::Request(args) => delegated::run(global, args, output).await,
        AppCommand::PrepareUpload {
            file,
            operation_id,
            parent_path,
        } => delegated::prepare_upload(*operation_id, parent_path, file, output).await,
        AppCommand::Transfer {
            upload_id,
            file,
            capability_file,
            capability_stdin,
        } => {
            delegated::transfer(
                global,
                *upload_id,
                file,
                capability_file.as_deref(),
                *capability_stdin,
                output,
            )
            .await
        }
    }
}

fn configure(global: &GlobalArgs, command: &ConfigCommand, output: Output) -> Result<()> {
    if let ConfigCommand::Home { location } = command {
        StateDirectory::configure_home(location)?;
        output.note(&format!(
            "Briefcase state will be stored under {}",
            location.join(".briefcase").display()
        ));
        return Ok(());
    }
    let state = StateDirectory::locate()?;
    // Writers lock before reading so concurrent setting changes merge.
    let _state_lock = match command {
        ConfigCommand::Set { .. } | ConfigCommand::Unset { .. } => Some(state.lock_credentials()?),
        ConfigCommand::Show | ConfigCommand::Home { .. } => None,
    };
    let mut configuration = state.configuration()?;
    let unknown = |key: &str| {
        CliError::usage(format!(
            "unknown setting `{key}`; the only setting is `telemetry` (Silicon Apps keeps briefcase up to date, so there is no update setting)"
        ))
    };
    match command {
        ConfigCommand::Show => {
            let profile = global
                .profile
                .as_deref()
                .unwrap_or(&configuration.current_profile);
            if output.is_json() {
                output.json(&serde_json::json!({
                    "profile": profile,
                    "telemetry": configuration.telemetry,
                    "state": state.path().display().to_string(),
                    "profiles": configuration.profiles,
                }));
            } else {
                println!("profile     {profile}");
                println!(
                    "telemetry   {}",
                    if configuration.telemetry { "on" } else { "off" }
                );
                println!("state       {}", state.path().display());
                for (name, saved) in &configuration.profiles {
                    println!("  {name:<10} {}", saved.url);
                }
            }
        }
        ConfigCommand::Set { key, value } if key == "telemetry" => {
            configuration.telemetry = parse_switch(value)?;
            state.save_configuration(&configuration)?;
            output.note(if configuration.telemetry {
                "telemetry enabled"
            } else {
                "telemetry disabled"
            });
        }
        ConfigCommand::Unset { key } if key == "telemetry" => {
            configuration.telemetry = true;
            state.save_configuration(&configuration)?;
            output.note("telemetry restored to the default: on");
        }
        ConfigCommand::Set { key, .. } | ConfigCommand::Unset { key } => return Err(unknown(key)),
        ConfigCommand::Home { .. } => {}
    }
    Ok(())
}

fn parse_switch(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(CliError::usage("setting must be on or off")),
    }
}

async fn version(global: &GlobalArgs, output: Output) -> Result<()> {
    let api_url = session::select(global).map_or_else(
        |_| session::normalize_api_url(global.url.as_deref().unwrap_or(session::DEFAULT_API_URL)),
        |selection| selection.api_url,
    );
    let served = match briefcase_client::Config::new(&api_url)
        .and_then(|config| Client::new_unchecked(config.with_telemetry(false)))
    {
        Ok(client) => client.version().await.ok(),
        Err(_) => None,
    };
    if output.is_json() {
        output.json(&serde_json::json!({
            "client": env!("CARGO_PKG_VERSION"),
            "client_api_version": briefcase_client::API_VERSION,
            "client_contract_version": briefcase_client::CONTRACT_VERSION,
            "server": served.as_ref().map(|version| serde_json::json!({
                "build": version.build,
                "contract_version": version.contract_version,
                "supported_api_versions": version.supported_api_versions,
            })),
        }));
        return Ok(());
    }
    println!(
        "briefcase {} (contract {}, API {})",
        env!("CARGO_PKG_VERSION"),
        briefcase_client::CONTRACT_VERSION,
        briefcase_client::API_VERSION
    );
    match served {
        Some(version) => println!(
            "server    build {} contract {} serving {}",
            version.build,
            version.contract_version,
            version.supported_api_versions.join(", ")
        ),
        None => println!("server    not reachable ({api_url})"),
    }
    Ok(())
}

async fn report(
    global: &GlobalArgs,
    message: String,
    pr: Option<String>,
    output: Output,
) -> Result<()> {
    if message.trim().is_empty() || message.len() > 16_384 {
        return Err(CliError::Usage("report message must contain 1–16384 bytes; include steps to reproduce, expected behavior, and actual behavior".into()));
    }
    let body = briefcase_client::BugReport {
        message,
        pr,
        isi: std::env::var("ISI")
            .ok()
            .filter(|value| !value.trim().is_empty()),
    };
    let context = session::connect(global).await?;
    let intent = serde_json::json!({
        "operation": "submit-report",
        "url": context.client.config().api_base().as_str(),
        "report": body,
    });
    let encoded =
        serde_json::to_vec(&intent).map_err(|error| CliError::Usage(error.to_string()))?;
    // Stable across process retries; the server scopes the key to the account.
    let key = IdempotencyKey::new(format!("report-{:x}", Sha256::digest(&encoded)))?;
    let receipt = context.client.submit_report_with_key(&body, &key).await?;
    output.json(&receipt);
    if body.pr.is_none() {
        eprintln!(
            "You can also propose a fix at https://github.com/teamofsilicons/silicon-briefcase and attach it with `briefcase report <message> --pr <pr-link>`."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{split_path, unique_next_cursor};

    #[test]
    fn a_path_splits_into_a_parent_and_a_name() {
        assert_eq!(
            split_path("notes/2026/onboarding"),
            (Some("notes/2026".to_owned()), "onboarding".to_owned())
        );
        assert_eq!(split_path("handbook"), (None, "handbook".to_owned()));
        assert_eq!(
            split_path("/notes/plans/"),
            (Some("notes".to_owned()), "plans".to_owned())
        );
    }

    #[test]
    fn exhaustive_pagination_rejects_a_repeated_cursor() {
        let mut seen = std::collections::HashSet::from(["first".to_owned()]);
        assert_eq!(
            unique_next_cursor(&mut seen, Some("second".to_owned())).unwrap(),
            Some("second".to_owned())
        );
        let error = unique_next_cursor(&mut seen, Some("first".to_owned())).unwrap_err();
        assert!(error.to_string().contains("repeated a pagination cursor"));
        assert_eq!(unique_next_cursor(&mut seen, None).unwrap(), None);
    }
}
