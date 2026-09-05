//! Turning a parsed command into calls on the package.

use std::{
    collections::HashSet,
    io::{IsTerminal as _, Read as _},
};

use briefcase_client::{
    AccessDecision, BucketConfiguration, Client, Config, Destination, Entry, EntryPage,
    EnvironmentKey, IamApplicationSecret, IamEnvironmentKey, IdempotencyKey, ListEntries,
    NewAccessRequest, NewFolder, NewGrant, OnBehalfOfUpload, PermissionQuery, TestingEnvironment,
    TestingEnvironmentCreate, TestingEnvironmentIamPairing, TestingEnvironmentUpdate, Upload,
    guess_content_type,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt as _;
use uuid::Uuid;

use crate::{
    cli::{
        AppCommand, BinCommand, Cli, Command, ConfigCommand, DecideArgs, DecisionArg, EnvCommand,
        FindArgs, GetArgs, GlobalArgs, LoginArgs, LsArgs, MkdirArgs, MvArgs, PutArgs, RestoreArgs,
        RmArgs, SearchArgs, ShareArgs, StorageCommand, SystemCommand, Target, TargetArgs,
        UnshareArgs,
    },
    render::{Output, human_size},
    state::{CredentialScope, PendingMutation, Profile, StateDirectory, StoredSession},
};

/// Everything that can stop a command, with the exit code it deserves.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Briefcase refused, or could not be reached.
    #[error(transparent)]
    Client(#[from] briefcase_client::Error),
    /// Checking or installing a crates.io release failed.
    #[error(transparent)]
    Update(#[from] briefcase_client::update::UpdateError),
    /// Local state could not be read or written.
    #[error(transparent)]
    State(#[from] crate::state::StateError),
    /// The command as typed cannot be carried out.
    #[error("{0}")]
    Usage(String),
    /// A local file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file involved.
        path: String,
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },
}

impl CliError {
    /// Returns the process exit code this failure deserves.
    ///
    /// Distinct codes let a script tell "not yours to see" from "the server is
    /// down" without parsing English.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Client(error) if error.is_not_found() => 3,
            Self::Client(error) if error.is_forbidden() || error.is_unauthenticated() => 4,
            _ => 1,
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}

type Result<T> = std::result::Result<T, CliError>;

/// Runs one parsed invocation.
///
/// # Errors
///
/// Returns whatever stopped the command, carrying the exit code it deserves.
pub async fn run(cli: Cli) -> Result<()> {
    let output = Output::new(cli.global.json);
    match cli.command {
        Command::Login(args) => login(&cli.global, &args, output).await,
        Command::Logout => logout(&cli.global, output),
        Command::Status => status(&cli.global, output).await,
        Command::Ls(args) => list(&cli.global, &args, output).await,
        Command::Find(args) => find(&cli.global, &args, output).await,
        Command::Search(args) => search(&cli.global, &args, output).await,
        Command::Stat(args) => stat(&cli.global, &args, output).await,
        Command::Mkdir(args) => mkdir(&cli.global, &args, output).await,
        Command::Put(args) => put(&cli.global, &args, output).await,
        Command::Get(args) => get(&cli.global, &args, output).await,
        Command::Cat(args) => cat(&cli.global, &args, output).await,
        Command::Mv(args) => move_entry(&cli.global, &args, output).await,
        Command::Rm(args) => remove(&cli.global, &args, output).await,
        Command::Bin(command) => bin(&cli.global, &command, output).await,
        Command::Versions(args) => versions(&cli.global, &args, output).await,
        Command::Restore(args) => restore_version(&cli.global, &args, output).await,
        Command::History(args) => history(&cli.global, &args, output).await,
        Command::Share(args) => share(&cli.global, &args, output).await,
        Command::Unshare(args) => unshare(&cli.global, &args, output).await,
        Command::Shares(args) => shares(&cli.global, &args, output).await,
        Command::Access(args) => access(&cli.global, &args.targets, output).await,
        Command::Request(args) => request_access(&cli.global, &args, output).await,
        Command::Decide(args) => decide(&cli.global, &args, output).await,
        Command::Inbox(args) => inbox(&cli.global, args.read, output).await,
        Command::Usage => usage(&cli.global, output).await,
        Command::Storage(command) => storage(&cli.global, &command, output).await,
        Command::App(command) => application(&cli.global, &command, output).await,
        Command::Env(command) => environment(&cli.global, &command, output).await,
        Command::Config(command) => configure(&cli.global, &command, output),
        Command::System(command) => system(&command, output).await,
        Command::Version => version(&cli.global, output).await,
    }
}

// ---- session ------------------------------------------------------------

/// Resolved deployment, plane, and authentication for one invocation.
struct ResolvedSession {
    profile_name: String,
    url: String,
    org: String,
    environment_id: Option<Uuid>,
    environment_key: Option<EnvironmentKey>,
    token: Option<String>,
    stored_session: Option<StoredSession>,
    credential_scope: CredentialScope,
}

fn session(global: &GlobalArgs) -> Result<ResolvedSession> {
    resolve_session(global, true)
}

/// Resolves only the destination and optional test root for an operation that
/// authenticates with its own request-scoped credential, such as an OBO proof.
fn anonymous_session(global: &GlobalArgs) -> Result<ResolvedSession> {
    resolve_session(global, false)
}

fn resolve_session(global: &GlobalArgs, include_bearer: bool) -> Result<ResolvedSession> {
    let state = StateDirectory::locate()?;
    let configuration = state.configuration()?;
    let profile_name = global
        .profile
        .clone()
        .unwrap_or(configuration.current_profile.clone());
    let saved = configuration.profiles.get(&profile_name);
    let url = global
        .url
        .clone()
        .or_else(|| saved.map(|profile| profile.url.clone()))
        .ok_or_else(|| {
            CliError::usage(
                "no deployment configured: run `briefcase login` or pass --url and --org",
            )
        })?;
    let org = global
        .org
        .clone()
        .or_else(|| saved.map(|profile| profile.org.clone()))
        .ok_or_else(|| {
            CliError::usage("no organization configured: run `briefcase login` or pass --org")
        })?;
    let credential_scope = scope_for(&url, &org)?;
    // A production OBO call needs no local credential material at all. Avoid
    // even deserializing an unrelated member session; only bearer-backed or
    // test-root-backed commands need the private credential store.
    let credentials = if include_bearer || global.test.is_some() {
        state.credentials()?
    } else {
        crate::state::Credentials::default()
    };
    let stored_session = include_bearer
        .then(|| credentials.session(&profile_name, global.test).cloned())
        .flatten();
    let legacy_token = (include_bearer && global.test.is_none())
        .then(|| credentials.tokens.get(&profile_name).cloned())
        .flatten();
    let environment_key = match global.test {
        Some(id) => {
            let key = credentials
                .testing_environment_key(&profile_name, id)
                .cloned()
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "testing environment {id} is not known for profile {profile_name}; run `briefcase env key {id}` first"
                    ))
                })?;
            ensure_stored_scope(
                &credentials,
                saved,
                &profile_name,
                Some(id),
                &credential_scope,
            )?;
            Some(key)
        }
        None => None,
    };
    if include_bearer
        && global.token.is_none()
        && (stored_session.is_some() || legacy_token.is_some())
    {
        ensure_stored_scope(
            &credentials,
            saved,
            &profile_name,
            global.test,
            &credential_scope,
        )?;
    }
    let token = include_bearer.then(|| {
        global
            .token
            .clone()
            .or_else(|| {
                stored_session
                    .as_ref()
                    .map(|session| session.access_token.clone())
            })
            .or(legacy_token)
    });

    Ok(ResolvedSession {
        profile_name,
        url,
        org,
        environment_id: global.test,
        environment_key,
        token: token.flatten(),
        stored_session,
        credential_scope,
    })
}

fn scope_for(url: &str, org: &str) -> Result<CredentialScope> {
    let config = Config::new(url, org)?;
    Ok(CredentialScope {
        deployment_origin: config.origin().as_str().to_owned(),
        organization: config.organization().to_owned(),
    })
}

fn ensure_stored_scope(
    credentials: &crate::state::Credentials,
    saved: Option<&Profile>,
    profile: &str,
    environment: Option<Uuid>,
    effective: &CredentialScope,
) -> Result<()> {
    let legacy_scope = saved
        .map(|saved| scope_for(&saved.url, &saved.org))
        .transpose()?;
    let bound = credentials
        .credential_scope(profile, environment)
        .or(legacy_scope.as_ref())
        .ok_or_else(|| {
            CliError::usage(format!(
                "stored credentials for profile {profile} have no destination binding; sign in again before using them"
            ))
        })?;
    if bound != effective {
        return Err(CliError::usage(format!(
            "stored credentials for profile {profile} are bound to {} organization {}; refusing to send them to {} organization {}",
            bound.deployment_origin,
            bound.organization,
            effective.deployment_origin,
            effective.organization,
        )));
    }
    Ok(())
}

fn testing_environment_for_login(
    credentials: &crate::state::Credentials,
    saved: Option<&Profile>,
    profile: &str,
    environment_id: Uuid,
    effective: &CredentialScope,
) -> Result<EnvironmentKey> {
    let environment = credentials
        .testing_environment_key(profile, environment_id)
        .cloned()
        .ok_or_else(|| {
            CliError::usage(format!(
                "testing environment {environment_id} is not known for profile {profile}; run `briefcase env key {environment_id}` first"
            ))
        })?;
    ensure_stored_scope(credentials, saved, profile, Some(environment_id), effective)?;
    Ok(environment)
}

fn config(session: &ResolvedSession) -> Result<Config> {
    let mut config = Config::new(&session.url, &session.org)?.with_auto_update(false);
    if let Some(environment) = &session.environment_key {
        config = config.with_environment(environment.clone());
    }
    Ok(config)
}

async fn connect(global: &GlobalArgs) -> Result<Client> {
    Ok(connect_resolved(global).await?.0)
}

async fn connect_resolved(global: &GlobalArgs) -> Result<(Client, ResolvedSession)> {
    let mut resolved = session(global)?;
    if global.token.is_none() {
        let state = StateDirectory::locate()?;
        let credentials_lock = state.lock_credentials()?;
        // Another process may have refreshed or logged out while this process
        // waited for the lock. Re-resolve from the authoritative atomic file.
        resolved = session(global)?;
        let mut credentials = state.credentials()?;
        if let Some(stored) = resolved.stored_session.clone()
            && stored.needs_refresh()
        {
            let mut pending = stored.clone();
            let refresh_key = pending
                .refresh_idempotency_key
                .clone()
                .unwrap_or_else(|| IdempotencyKey::random().as_str().to_owned());
            if pending.refresh_idempotency_key.is_none() {
                pending.refresh_idempotency_key = Some(refresh_key.clone());
                credentials.set_session(
                    &resolved.profile_name,
                    resolved.environment_id,
                    pending.clone(),
                );
                credentials.set_credential_scope(
                    &resolved.profile_name,
                    resolved.environment_id,
                    resolved.credential_scope.clone(),
                );
                state.save_credentials(&credentials)?;
                resolved.stored_session = Some(pending);
            }
            let refresh_key = IdempotencyKey::new(refresh_key)?;
            let refresh_client = if global.no_verify {
                Client::new_unchecked(config(&resolved)?)?
            } else {
                // Verify the destination before presenting a rotating,
                // single-use refresh credential to it.
                Client::connect(config(&resolved)?).await?
            };
            let refreshed = refresh_client
                .refresh_session_with_key(&stored.refresh_token, &refresh_key)
                .await?;
            let refreshed = StoredSession::from_tokens(&refreshed);
            credentials.set_session(
                &resolved.profile_name,
                resolved.environment_id,
                refreshed.clone(),
            );
            credentials.set_credential_scope(
                &resolved.profile_name,
                resolved.environment_id,
                resolved.credential_scope.clone(),
            );
            state.save_credentials(&credentials)?;
            resolved.token = Some(refreshed.access_token.clone());
            resolved.stored_session = Some(refreshed);
        }
        drop(credentials_lock);
    }

    let mut config = config(&resolved)?;
    if let Some(token) = resolved.token.clone() {
        config = config.with_token(token);
    }
    let client = if global.no_verify {
        Client::new_unchecked(config)?
    } else {
        Client::connect(config).await?
    };
    Ok((client, resolved))
}

/// Resolves a target to an entry identifier, looking a path up when needed.
async fn entry_id(client: &Client, target: &Target) -> Result<Uuid> {
    match target {
        Target::Id(id) => Ok(*id),
        Target::Path(path) => Ok(client.entry_at(path).await?.id),
    }
}

/// Splits `a/b/c` into the parent path `a/b` and the name `c`.
fn split_path(path: &str) -> (Option<String>, String) {
    let trimmed = path.trim_matches('/');
    trimmed.rsplit_once('/').map_or_else(
        || (None, trimmed.to_owned()),
        |(parent, name)| (Some(parent.to_owned()), name.to_owned()),
    )
}

// ---- commands -----------------------------------------------------------

#[allow(
    clippy::too_many_lines,
    reason = "login keeps its locked one-time exchange and atomic state transition together"
)]
async fn login(global: &GlobalArgs, args: &LoginArgs, output: Output) -> Result<()> {
    if global.token.is_some() {
        return Err(CliError::usage(
            "--token is an access-token override, not a login method; give `briefcase login` an IAM short-lived token",
        ));
    }
    let state = StateDirectory::locate()?;
    let url = global
        .url
        .clone()
        .ok_or_else(|| CliError::usage("--url is required to log in"))?;
    let org = global
        .org
        .clone()
        .ok_or_else(|| CliError::usage("--org is required to log in"))?;
    let slt = if args.slt_stdin {
        read_secret_stdin()?
    } else {
        match &args.slt {
            Some(slt) => slt.clone(),
            None => prompt_secret("IAM short-lived token: ")?,
        }
    };
    if slt.is_empty() {
        return Err(CliError::usage("the short-lived token was empty"));
    }

    // A login can replace a rotating session. Serialize the complete
    // read/exchange/write sequence so concurrent logins and refreshes cannot
    // resurrect an older token pair. Secret input happens before taking the
    // lock, so an unattended prompt cannot block another CLI process.
    let credentials_lock = state.lock_credentials()?;
    let mut configuration = state.configuration()?;
    let profile_name = args
        .save_as
        .clone()
        .or_else(|| global.profile.clone())
        .unwrap_or_else(|| configuration.current_profile.clone());
    let mut credentials = state.credentials()?;
    if let Some(previous) = configuration.profiles.get(&profile_name) {
        let previous_scope = scope_for(&previous.url, &previous.org)?;
        credentials.bind_legacy_profile_scope(&profile_name, &previous_scope);
    }
    let login_scope = scope_for(&url, &org)?;
    let mut login_config = Config::new(&url, &org)?.with_auto_update(false);
    if let Some(environment_id) = global.test {
        let environment = testing_environment_for_login(
            &credentials,
            configuration.profiles.get(&profile_name),
            &profile_name,
            environment_id,
            &login_scope,
        )?;
        login_config = login_config.with_environment(environment);
    }
    let client = if global.no_verify {
        Client::new_unchecked(login_config)?
    } else {
        Client::connect(login_config).await?
    };
    let scope = format!("login:{}", plane_scope(&profile_name, global.test));
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "login",
        "profile": profile_name,
        "url": url,
        "org": org,
        "testing_environment_id": global.test,
        "slt": slt,
    }))?;
    let pending = prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
    let idempotency_key = IdempotencyKey::new(pending.idempotency_key.clone())?;
    let tokens = client
        .login_with_slt_with_key(&slt, &idempotency_key)
        .await?;
    let stored = StoredSession::from_tokens(&tokens);

    configuration.profiles.insert(
        profile_name.clone(),
        Profile {
            url: url.clone(),
            org: org.clone(),
        },
    );
    configuration.current_profile.clone_from(&profile_name);
    state.save_configuration(&configuration)?;

    credentials.set_session(&profile_name, global.test, stored.clone());
    credentials.set_credential_scope(&profile_name, global.test, login_scope);
    if global.test.is_none() {
        credentials.tokens.remove(&profile_name);
    }
    credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
    state.save_credentials(&credentials)?;
    drop(credentials_lock);

    if output.is_json() {
        output.json(&serde_json::json!({
            "profile": profile_name,
            "url": url,
            "org": org,
            "test_environment_id": global.test,
            "actor": stored.actor,
            "expires_at": stored.expires_at,
        }));
    } else {
        output.note(&format!(
            "signed in as {}:{} on profile {}{}\nrotating session stored in {}",
            stored.actor.actor_type.as_str(),
            stored.actor.public_id,
            profile_name,
            global
                .test
                .map_or_else(String::new, |id| format!(" in test environment {id}")),
            state.credentials_path().display()
        ));
    }
    Ok(())
}

fn plane_scope(profile: &str, environment: Option<Uuid>) -> String {
    environment.map_or_else(
        || format!("{profile}:production"),
        |id| format!("{profile}:test:{id}"),
    )
}

fn request_fingerprint(intent: &impl Serialize) -> Result<String> {
    let body = serde_json::to_vec(intent)
        .map_err(|error| CliError::usage(format!("could not identify retry intent: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(body)))
}

fn prepare_pending_mutation(
    state: &StateDirectory,
    credentials: &mut crate::state::Credentials,
    scope: &str,
    fingerprint: &str,
    expected_version: Option<i64>,
) -> Result<PendingMutation> {
    let pending = credentials.prepare_mutation(scope, fingerprint, expected_version);
    // Save even when this is a retry: a previous CLI release may have loaded
    // legacy state, and the fsync is the boundary before the mutation begins.
    state.save_credentials(credentials)?;
    Ok(pending)
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

/// Persists a normal content mutation before allowing its request to start.
///
/// Unlike generated-secret lifecycle changes, these requests do not need the
/// state lock held while the network is slow: a matching concurrent command
/// may safely reuse the same server-side idempotency identity.
fn prepare_durable_mutation(
    scope: &str,
    fingerprint: &str,
    resource_id: Option<Uuid>,
    destination_id: Option<Uuid>,
) -> Result<PendingMutation> {
    let state = StateDirectory::locate()?;
    let _state_lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    let mut pending = credentials.prepare_mutation(scope, fingerprint, None);
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

fn destination_identity(destination: &Destination) -> String {
    match destination {
        Destination::Id(id) => format!("id:{id}"),
        Destination::Path(path) => format!("path:{path}"),
    }
}

async fn stable_destination_id(
    client: &Client,
    destination: &Destination,
    recovered: Option<&PendingMutation>,
) -> Result<Uuid> {
    if let Some(id) = recovered.and_then(|pending| pending.destination_id) {
        return Ok(id);
    }
    match destination {
        Destination::Id(id) => Ok(*id),
        Destination::Path(path) => Ok(client.entry_at(path).await?.id),
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

fn prompt_secret(label: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password(label).map_err(|source| CliError::Io {
            path: "terminal".to_owned(),
            source,
        });
    }
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|source| CliError::Io {
            path: "standard input".to_owned(),
            source,
        })?;
    Ok(line.trim().to_owned())
}

fn read_secret_stdin() -> Result<String> {
    let mut buffer = String::new();
    std::io::stdin()
        .read_to_string(&mut buffer)
        .map_err(|source| CliError::Io {
            path: "standard input".to_owned(),
            source,
        })?;
    Ok(buffer.trim().to_owned())
}

fn logout(global: &GlobalArgs, output: Output) -> Result<()> {
    let state = StateDirectory::locate()?;
    let configuration = state.configuration()?;
    let profile = global
        .profile
        .clone()
        .unwrap_or(configuration.current_profile.clone());
    let _credentials_lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    let had_session = credentials.remove_session(&profile, global.test);
    let had_legacy_token = if global.test.is_none() {
        credentials.tokens.remove(&profile).is_some()
    } else {
        false
    };
    state.save_credentials(&credentials)?;
    output.note(if had_session || had_legacy_token {
        "session removed"
    } else {
        "no session was stored for this profile and plane"
    });
    Ok(())
}

async fn status(global: &GlobalArgs, output: Output) -> Result<()> {
    let session = session(global)?;
    let mut client_config = config(&session)?;
    if let Some(token) = session.token.clone() {
        client_config = client_config.with_token(token);
    }
    let client = Client::new_unchecked(client_config)?;
    let served = client.version().await?;
    let agreement = served.check_compatibility();

    if output.is_json() {
        output.json(&serde_json::json!({
            "profile": session.profile_name,
            "url": session.url,
            "org": session.org,
            "test_environment_id": session.environment_id,
            "authenticated": session.token.is_some(),
            "session_expires_at": session.stored_session.as_ref().map(|stored| stored.expires_at),
            "service": served.service,
            "build": served.build,
            "contract_version": served.contract_version,
            "supported_api_versions": served.supported_api_versions,
            "compatible": agreement.is_ok(),
        }));
        return Ok(());
    }
    println!("profile     {}", session.profile_name);
    println!("state       {}", StateDirectory::locate()?.path().display());
    println!("deployment  {}", session.url);
    println!("organization {}", session.org);
    println!(
        "plane       {}",
        session
            .environment_id
            .map_or_else(|| "production".to_owned(), |id| format!("test {id}"))
    );
    println!(
        "session     {}",
        if session.token.is_some() {
            "stored"
        } else {
            "none — commands that need one will be refused"
        }
    );
    println!(
        "service     {} build {} contract {}",
        served.service, served.build, served.contract_version
    );
    match agreement {
        Ok(()) => println!("contract    agreed on {}", served.selected_api_version),
        Err(error) => println!("contract    MISMATCH: {error}"),
    }
    Ok(())
}

async fn list(global: &GlobalArgs, args: &LsArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let query = ListEntries {
        parent: args.target.as_ref().map(Target::destination),
        cursor: args.cursor.clone(),
        limit: args.limit,
        ..ListEntries::default()
    };
    let page = if args.all {
        EntryPage {
            items: list_every_entry(&client, query).await?,
            next_cursor: None,
        }
    } else {
        client.list_entries(&query).await?
    };
    output.entry_page(&page, args.long);
    Ok(())
}

async fn find(global: &GlobalArgs, args: &FindArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let query = ListEntries {
        parent: args.in_folder.as_ref().map(Target::destination),
        cursor: args.cursor.clone(),
        limit: args.limit,
        ..ListEntries::matching(&args.filter)
    };
    let page = if args.all {
        EntryPage {
            items: list_every_entry(&client, query).await?,
            next_cursor: None,
        }
    } else {
        client.list_entries(&query).await?
    };
    output.entry_page(&page, true);
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
    let client = connect(global).await?;
    let results = client.search(&args.query, args.limit).await?;
    output.search(&results);
    Ok(())
}

async fn stat(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let entry = resolve_entry(&client, &args.target).await?;
    output.entry(&entry);
    Ok(())
}

async fn resolve_entry(client: &Client, target: &Target) -> Result<Entry> {
    match target {
        Target::Id(id) => Ok(client.entry(*id).await?),
        Target::Path(path) => Ok(client.entry_at(path).await?),
    }
}

async fn mkdir(global: &GlobalArgs, args: &MkdirArgs, output: Output) -> Result<()> {
    let (client, resolved) = connect_resolved(global).await?;
    let (parent, name) = split_path(&args.path);
    if name.is_empty() {
        return Err(CliError::usage("a folder name is required"));
    }
    let mut folder = if let Some(parent) = parent {
        NewFolder::in_folder(name, Destination::path(parent))
    } else {
        let root_type = args.root_type.ok_or_else(|| {
            CliError::usage("a folder at the organization base needs --type public, private or tag")
        })?;
        let mut folder = NewFolder::at_base(name, root_type.into());
        folder.tag.clone_from(&args.tag);
        if matches!(root_type, crate::cli::RootTypeArg::Tag) && folder.tag.is_none() {
            return Err(CliError::usage("--tag is required for a tag folder"));
        }
        folder
    };
    folder.invitees = args
        .invites
        .iter()
        .map(|invitation| {
            NewGrant::new(invitation.principal.clone(), invitation.access.clone()).inheriting()
        })
        .collect();

    let parent = folder.parent.as_ref().map(destination_identity);
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "create-folder",
        "profile": &resolved.profile_name,
        "url": &resolved.url,
        "org": &resolved.org,
        "testing_environment_id": resolved.environment_id,
        "name": &folder.name,
        "parent": parent,
        "root_type": folder.root_type,
        "tag": &folder.tag,
        "invitees": &folder.invitees,
    }))?;
    let address = request_fingerprint(&serde_json::json!({
        "path": &args.path,
        "testing_environment_id": resolved.environment_id,
    }))?;
    let scope = format!(
        "entry:mkdir:{}:{address}",
        plane_scope(&resolved.profile_name, resolved.environment_id)
    );
    let recovered = durable_pending(&scope, &fingerprint)?;
    let destination_id = match &folder.parent {
        Some(destination) => {
            Some(stable_destination_id(&client, destination, recovered.as_ref()).await?)
        }
        None => None,
    };
    let pending = prepare_durable_mutation(&scope, &fingerprint, None, destination_id)?;
    if folder.parent.is_some() {
        folder.parent = Some(Destination::Id(pending.destination_id.ok_or_else(
            || CliError::usage("the pending folder creation has no destination identifier"),
        )?));
    }
    folder.idempotency_key = Some(IdempotencyKey::new(pending.idempotency_key.clone())?);
    let created = client.create_folder(&folder).await?;
    finish_durable_mutation(&scope, &pending)?;
    if output.is_json() {
        output.json(&created);
    } else {
        output.note(&format!("created {}", created.path));
    }
    Ok(())
}

async fn put(global: &GlobalArgs, args: &PutArgs, output: Output) -> Result<()> {
    if args.name.is_some() && args.sources.len() > 1 {
        return Err(CliError::usage("--name works with a single file"));
    }
    let (client, resolved) = connect_resolved(global).await?;
    let mut stored = Vec::with_capacity(args.sources.len());
    for source in &args.sources {
        let mut upload = Upload::file(args.destination.destination(), source.clone())?;
        if let Some(name) = &args.name {
            upload = upload.named(name.clone());
        }
        let content_type = args
            .content_type
            .clone()
            .unwrap_or_else(|| guess_content_type(&upload.file_name).to_owned());
        upload = upload.with_content_type(content_type);

        let (canonical_source, content_sha256) = file_identity(source).await?;
        let destination = destination_identity(&upload.destination);
        let address = request_fingerprint(&serde_json::json!({
            "source": &canonical_source,
            "destination": &destination,
            "file_name": &upload.file_name,
            "testing_environment_id": resolved.environment_id,
        }))?;
        let scope = format!(
            "entry:put:{}:{address}",
            plane_scope(&resolved.profile_name, resolved.environment_id)
        );
        let fingerprint = request_fingerprint(&serde_json::json!({
            "operation": "upload-file",
            "profile": &resolved.profile_name,
            "url": &resolved.url,
            "org": &resolved.org,
            "testing_environment_id": resolved.environment_id,
            "source": canonical_source,
            "destination": destination,
            "file_name": &upload.file_name,
            "content_type": &upload.content_type,
            "content_sha256": content_sha256,
        }))?;
        let recovered = durable_pending(&scope, &fingerprint)?;
        let destination_id =
            stable_destination_id(&client, &upload.destination, recovered.as_ref()).await?;
        let pending = prepare_durable_mutation(&scope, &fingerprint, None, Some(destination_id))?;
        upload.destination =
            Destination::Id(pending.destination_id.ok_or_else(|| {
                CliError::usage("the pending upload has no destination identifier")
            })?);
        upload.idempotency_key = Some(IdempotencyKey::new(pending.idempotency_key.clone())?);
        let entry = client.upload(&upload).await?;
        finish_durable_mutation(&scope, &pending)?;
        if !output.is_json() {
            println!(
                "{} → {} ({})",
                source.display(),
                entry.path,
                entry.size.map_or_else(|| "-".to_owned(), human_size)
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
    let client = connect(global).await?;
    let entry = resolve_entry(&client, &args.target).await?;
    let destination = args
        .output
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(&entry.name));
    let written = client.download_to_file(entry.id, &destination).await?;
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
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    let mut stream = client.read_content(id, None).await?;
    while let Some(chunk) = stream.chunk().await? {
        output.bytes(&chunk).map_err(|source| CliError::Io {
            path: "standard output".to_owned(),
            source,
        })?;
    }
    Ok(())
}

async fn move_entry(global: &GlobalArgs, args: &MvArgs, output: Output) -> Result<()> {
    let (parent_path, name) = split_path(&args.destination);
    if name.is_empty() {
        return Err(CliError::usage("a destination name is required"));
    }
    let (client, resolved) = connect_resolved(global).await?;
    let address = request_fingerprint(&serde_json::json!({
        "target": args.target.to_string(),
        "destination": &args.destination,
        "testing_environment_id": resolved.environment_id,
    }))?;
    let scope = format!(
        "entry:move:{}:{address}",
        plane_scope(&resolved.profile_name, resolved.environment_id)
    );
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "update-entry",
        "profile": &resolved.profile_name,
        "url": &resolved.url,
        "org": &resolved.org,
        "testing_environment_id": resolved.environment_id,
        "target": args.target.to_string(),
        "destination": &args.destination,
    }))?;
    let recovered = durable_pending(&scope, &fingerprint)?;
    let (resource_id, original_path) =
        if let Some(id) = recovered.as_ref().and_then(|pending| pending.resource_id) {
            (id, args.target.to_string())
        } else {
            let entry = resolve_entry(&client, &args.target).await?;
            (entry.id, entry.path)
        };
    let destination_id = match parent_path {
        Some(parent_path) => {
            let destination = Destination::path(parent_path);
            Some(stable_destination_id(&client, &destination, recovered.as_ref()).await?)
        }
        None => None,
    };
    let pending =
        prepare_durable_mutation(&scope, &fingerprint, Some(resource_id), destination_id)?;
    let entry_id = pending.resource_id.unwrap_or(resource_id);
    let mut update = briefcase_client::EntryUpdate::rename(name);
    update.parent_id = pending.destination_id;
    let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
    let moved = client
        .update_entry_with_key(entry_id, &update, &key)
        .await?;
    finish_durable_mutation(&scope, &pending)?;
    if output.is_json() {
        output.json(&moved);
    } else {
        output.note(&format!("{} → {}", original_path, moved.path));
    }
    Ok(())
}

async fn remove(global: &GlobalArgs, args: &RmArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    for target in &args.targets {
        let entry = resolve_entry(&client, target).await?;
        client.delete_entry(entry.id).await?;
        output.note(&format!(
            "{} moved to the bin, recoverable for 45 days ({})",
            entry.path, entry.id
        ));
    }
    Ok(())
}

async fn bin(global: &GlobalArgs, command: &BinCommand, output: Output) -> Result<()> {
    let client = connect(global).await?;
    match command {
        BinCommand::List { limit, cursor, all } => {
            let page = if *all {
                EntryPage {
                    items: list_every_bin_entry(&client, cursor.clone(), *limit).await?,
                    next_cursor: None,
                }
            } else {
                client.bin(cursor.as_deref(), *limit).await?
            };
            output.entry_page(&page, true);
        }
        BinCommand::Restore { entry_id } => {
            let entry = client.restore_from_bin(*entry_id).await?;
            if output.is_json() {
                output.json(&entry);
            } else {
                output.note(&format!("restored to {}", entry.path));
            }
        }
    }
    Ok(())
}

async fn list_every_bin_entry(
    client: &Client,
    mut cursor: Option<String>,
    limit: Option<u16>,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut seen_cursors = HashSet::new();
    if let Some(cursor) = &cursor {
        seen_cursors.insert(cursor.clone());
    }
    loop {
        let page = client.bin(cursor.as_deref(), limit).await?;
        entries.extend(page.items);
        let Some(next_cursor) = unique_next_cursor(&mut seen_cursors, page.next_cursor)? else {
            return Ok(entries);
        };
        cursor = Some(next_cursor);
    }
}

async fn versions(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    let versions = client.versions(id).await?;
    output.versions(&versions);
    Ok(())
}

async fn restore_version(global: &GlobalArgs, args: &RestoreArgs, output: Output) -> Result<()> {
    let (client, resolved) = connect_resolved(global).await?;
    let address = request_fingerprint(&serde_json::json!({
        "target": args.target.to_string(),
        "version_id": args.version_id,
        "testing_environment_id": resolved.environment_id,
    }))?;
    let scope = format!(
        "entry:restore-version:{}:{address}",
        plane_scope(&resolved.profile_name, resolved.environment_id)
    );
    let fingerprint = request_fingerprint(&serde_json::json!({
        "operation": "restore-version",
        "profile": &resolved.profile_name,
        "url": &resolved.url,
        "org": &resolved.org,
        "testing_environment_id": resolved.environment_id,
        "target": args.target.to_string(),
        "version_id": args.version_id,
    }))?;
    let id = match pending_resource_id(&scope, &fingerprint)? {
        Some(id) => id,
        None => entry_id(&client, &args.target).await?,
    };
    let pending = prepare_durable_mutation(&scope, &fingerprint, Some(id), None)?;
    let id = pending.resource_id.unwrap_or(id);
    let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
    let entry = client
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

async fn history(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    let events = client.activity(id).await?;
    output.history(&events);
    Ok(())
}

async fn share(global: &GlobalArgs, args: &ShareArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    let mut grant = NewGrant::new(args.principal.0.clone(), args.access.0.clone());
    if args.inherit {
        grant = grant.inheriting();
    }
    let created = client.grant(id, &grant).await?;
    if output.is_json() {
        output.json(&created);
    } else {
        output.note(&format!(
            "{} may now {} {}{}",
            created.principal,
            created
                .access
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            args.target,
            if created.inherit {
                " and everything inside it"
            } else {
                ""
            }
        ));
    }
    Ok(())
}

async fn unshare(global: &GlobalArgs, args: &UnshareArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    client.revoke(id, args.grant_id).await?;
    output.note("grant revoked; access from a tag, Public visibility, ownership or administration is untouched");
    Ok(())
}

async fn shares(global: &GlobalArgs, args: &TargetArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let id = entry_id(&client, &args.target).await?;
    let grants = client.permissions(id).await?;
    output.grants(&grants);
    Ok(())
}

async fn access(global: &GlobalArgs, targets: &[Target], output: Output) -> Result<()> {
    let client = connect(global).await?;
    let mut query = PermissionQuery::default();
    for target in targets {
        match target {
            Target::Id(id) => query.entry_ids.push(*id),
            Target::Path(path) => query.paths.push(path.clone()),
        }
    }
    let inspection = client.effective_access(&query).await?;
    output.inspection(&inspection);
    Ok(())
}

async fn request_access(
    global: &GlobalArgs,
    args: &crate::cli::RequestArgs,
    output: Output,
) -> Result<()> {
    let (client, resolved) = connect_resolved(global).await?;
    let mut request = NewAccessRequest::new(args.access.0.clone());
    if let Some(reason) = &args.reason {
        request = request.because(reason.clone());
    }
    let created = match &args.target {
        Target::Id(id) => client.request_access(*id, &request).await?,
        Target::Path(path) => {
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "request-access-by-path",
                "profile": &resolved.profile_name,
                "url": &resolved.url,
                "org": &resolved.org,
                "testing_environment_id": resolved.environment_id,
                "path": path,
                "access": &request.access,
                "reason": &request.reason,
            }))?;
            let address = request_fingerprint(&serde_json::json!({
                "path": path,
                "testing_environment_id": resolved.environment_id,
            }))?;
            let scope = format!(
                "access:request:{}:{address}",
                plane_scope(&resolved.profile_name, resolved.environment_id)
            );
            let pending = prepare_durable_mutation(&scope, &fingerprint, None, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let created = client
                .request_access_by_path_with_key(path, &request, &key)
                .await?;
            finish_durable_mutation(&scope, &pending)?;
            created
        }
    };
    output.access_request(&created);
    Ok(())
}

async fn decide(global: &GlobalArgs, args: &DecideArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let decision = match args.decision {
        DecisionArg::Approve => AccessDecision::Approve(args.access.0.clone()),
        DecisionArg::Deny => AccessDecision::Deny,
    };
    let decided = client
        .decide_access_request(args.request_id, &decision)
        .await?;
    output.access_request(&decided);
    Ok(())
}

async fn inbox(global: &GlobalArgs, mark_read: bool, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let inbox = if mark_read {
        client.mark_notifications_read().await?
    } else {
        client.notifications().await?
    };
    output.inbox(&inbox);
    Ok(())
}

async fn usage(global: &GlobalArgs, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let usage = client.usage().await?;
    output.usage(&usage);
    Ok(())
}

async fn storage(global: &GlobalArgs, command: &StorageCommand, output: Output) -> Result<()> {
    let client = connect(global).await?;
    let StorageCommand::Configure(args) = command;
    let configuration = BucketConfiguration {
        bucket_name: args.bucket.clone(),
        region: args.region.clone(),
        role_arn: args.role_arn.clone(),
        prefix: args.prefix.clone(),
        aws_account_id: args.account.clone(),
        encryption_mode: args.encryption.into(),
        kms_key_arn: args.kms_key_arn.clone(),
    };
    let status = client.configure_storage(&configuration).await?;
    if output.is_json() {
        output.json(&status);
    } else {
        match status.status {
            briefcase_client::BucketConfigurationState::Configured => {
                println!(
                    "configured; the organization's files now go to {}",
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
    Ok(())
}

async fn application(global: &GlobalArgs, command: &AppCommand, output: Output) -> Result<()> {
    // An OBO proof is the complete actor credential for this operation. Never
    // load or rotate an unrelated member session that will not be sent.
    let session = anonymous_session(global)?;
    let client = if global.no_verify {
        Client::new_unchecked(config(&session)?)?
    } else {
        Client::connect(config(&session)?).await?
    };
    let AppCommand::Upload {
        app_id,
        proof,
        proof_stdin,
        file,
    } = command;
    let proof = if *proof_stdin {
        read_secret_stdin()?
    } else {
        match proof {
            Some(proof) => proof.clone(),
            None => prompt_secret("IAM OBO access proof: ")?,
        }
    };
    if proof.is_empty() {
        return Err(CliError::usage("the OBO access proof was empty"));
    }
    let entry = client
        .create_file_on_behalf_of(&OnBehalfOfUpload::file(
            app_id.to_string(),
            proof,
            file.clone(),
        ))
        .await?;
    if output.is_json() {
        output.json(&entry);
    } else {
        output.note(&format!("created {}", entry.path));
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "the lifecycle verbs share state and rendering; one match keeps their policy together"
)]
async fn environment(global: &GlobalArgs, command: &EnvCommand, output: Output) -> Result<()> {
    if matches!(
        command,
        EnvCommand::Current
            | EnvCommand::Clean {
                environment_id: None
            }
    ) {
        require_test(global)?;
        let session = session(global)?;
        let client = if global.no_verify {
            Client::new_unchecked(config(&session)?)?
        } else {
            Client::connect(config(&session)?).await?
        };
        return match command {
            EnvCommand::Current => {
                let current = client.current_testing_environment().await?;
                if output.is_json() {
                    output.json(&current);
                } else {
                    println!("id             {}", current.id);
                    println!("name           {}", current.name);
                    println!(
                        "description    {}",
                        current.description.as_deref().unwrap_or("-")
                    );
                    println!("key generation {}", current.key_generation);
                    println!("created        {}", current.created_at);
                }
                Ok(())
            }
            EnvCommand::Clean {
                environment_id: None,
            } => {
                let environment_id = require_test(global)?;
                let state = StateDirectory::locate()?;
                let credentials_lock = state.lock_credentials()?;
                let mut credentials = state.credentials()?;
                let scope = format!(
                    "env:clean-current:{}",
                    plane_scope(&session.profile_name, Some(environment_id))
                );
                let fingerprint = request_fingerprint(&serde_json::json!({
                    "operation": "clean-current-testing-environment",
                    "profile": session.profile_name,
                    "url": session.url,
                    "org": session.org,
                    "environment_id": environment_id,
                    "environment_key": session.environment_key,
                }))?;
                let pending =
                    prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
                let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
                let cleaned = client
                    .clean_current_testing_environment_with_key(&key)
                    .await?;
                credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
                state.save_credentials(&credentials)?;
                drop(credentials_lock);
                report_cleaning(output, &cleaned);
                Ok(())
            }
            _ => unreachable!(),
        };
    }

    if global.test.is_some() {
        return Err(CliError::usage(
            "testing-environment management commands run in production; use `env current` or UUID-less `env clean` with --test",
        ));
    }

    let (client, management_session) = connect_resolved(global).await?;
    let profile = management_session.profile_name;
    let management_scope = management_session.credential_scope;
    match command {
        EnvCommand::List { status } => {
            let page = client.testing_environments(status.as_deref()).await?;
            if output.is_json() {
                output.json(&page);
            } else if page.items.is_empty() {
                output.note("(no testing environments)");
            } else {
                for environment in &page.items {
                    println!(
                        "{}  {:<20}  {:?}  key generation {}",
                        environment.id,
                        environment.name,
                        environment.status,
                        environment.key_generation
                    );
                }
            }
        }
        EnvCommand::Create {
            name,
            description,
            iam_environment_id,
            iam_environment_key,
            iam_app_id,
            iam_app_secret,
        } => {
            let iam_environment_key = match iam_environment_key {
                Some(key) => IamEnvironmentKey::new(key.clone())?,
                None => IamEnvironmentKey::new(prompt_secret("IAM environment root key: ")?)?,
            };
            let iam_app_secret = match iam_app_secret {
                Some(secret) => IamApplicationSecret::new(secret.clone())?,
                None => IamApplicationSecret::new(prompt_secret("IAM Application secret: ")?)?,
            };
            let mut input = TestingEnvironmentCreate::new(
                name,
                *iam_environment_id,
                iam_environment_key,
                iam_app_id.clone(),
                iam_app_secret,
            );
            input.description.clone_from(description);
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:create:{profile}:{name}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "create-testing-environment",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "input": input,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let created = client
                .create_testing_environment_with_key(&input, &key)
                .await?;
            credentials.set_testing_environment_key(
                &profile,
                created.environment.id,
                created.key.clone(),
            );
            credentials.set_credential_scope(
                &profile,
                Some(created.environment.id),
                management_scope.clone(),
            );
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            if output.is_json() {
                output.json(&created);
            } else {
                println!(
                    "created {} ({})",
                    created.environment.name, created.environment.id
                );
                println!("key: {}", created.key.expose_secret());
                println!(
                    "this device stored it; use `briefcase --test {} <command>`",
                    created.environment.id
                );
            }
        }
        EnvCommand::Show { environment_id } => {
            let value = client.testing_environment(*environment_id).await?;
            report_environment(output, &value);
        }
        EnvCommand::Update {
            environment_id,
            name,
            description,
            clear_description,
        } => {
            let update = TestingEnvironmentUpdate {
                name: name.clone(),
                description: if *clear_description {
                    Some(None)
                } else {
                    description.clone().map(Some)
                },
            };
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:update:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "update-testing-environment",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
                "update": update,
            }))?;
            let expected_version = credentials
                .pending_mutation(&scope)
                .filter(|pending| pending.request_fingerprint == fingerprint)
                .and_then(|pending| pending.expected_version);
            let expected_version = match expected_version {
                Some(version) => version,
                None => client.testing_environment(*environment_id).await?.version,
            };
            let pending = prepare_pending_mutation(
                &state,
                &mut credentials,
                &scope,
                &fingerprint,
                Some(expected_version),
            )?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .update_testing_environment_with_key(
                    *environment_id,
                    pending.expected_version.ok_or_else(|| {
                        CliError::usage("the pending environment update has no expected version")
                    })?,
                    &update,
                    &key,
                )
                .await?;
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            report_environment(output, &value);
        }
        EnvCommand::Delete { environment_id } => {
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:delete:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "delete-testing-environment",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .delete_testing_environment_with_key(*environment_id, &key)
                .await?;
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            report_environment(output, &value);
        }
        EnvCommand::Restore { environment_id } => {
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:restore:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "restore-testing-environment",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .restore_testing_environment_with_key(*environment_id, &key)
                .await?;
            credentials.set_testing_environment_key(&profile, *environment_id, value.key.clone());
            credentials.set_credential_scope(
                &profile,
                Some(*environment_id),
                management_scope.clone(),
            );
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            if output.is_json() {
                output.json(&value);
            } else {
                report_environment(output, &value.environment);
                println!("new key: {}", value.key.expose_secret());
                println!("this device stored the replacement key");
            }
        }
        EnvCommand::Key { environment_id } => {
            // Keep a key fetch and its local replacement in the same critical
            // section as rotation. Otherwise an earlier GET could arrive after
            // a concurrent rotation and overwrite the new root with a stale one.
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let value = client.testing_environment_key(*environment_id).await?;
            credentials.set_testing_environment_key(&profile, *environment_id, value.key.clone());
            credentials.set_credential_scope(
                &profile,
                Some(*environment_id),
                management_scope.clone(),
            );
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            if output.is_json() {
                output.json(&value);
            } else {
                println!("{}", value.key.expose_secret());
            }
        }
        EnvCommand::RotateKey { environment_id } => {
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:rotate-key:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "rotate-testing-environment-key",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .rotate_testing_environment_key_with_key(*environment_id, &key)
                .await?;
            credentials.set_testing_environment_key(&profile, *environment_id, value.key.clone());
            credentials.set_credential_scope(
                &profile,
                Some(*environment_id),
                management_scope.clone(),
            );
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            if output.is_json() {
                output.json(&value);
            } else {
                println!("key: {}", value.key.expose_secret());
                println!("the previous key stopped working");
            }
        }
        EnvCommand::PairIam {
            environment_id,
            iam_environment_id,
            iam_environment_key,
            iam_app_id,
            iam_app_secret,
        } => {
            let iam_environment_key = match iam_environment_key {
                Some(key) => IamEnvironmentKey::new(key.clone())?,
                None => IamEnvironmentKey::new(prompt_secret("IAM environment root key: ")?)?,
            };
            let iam_app_secret = match iam_app_secret {
                Some(secret) => IamApplicationSecret::new(secret.clone())?,
                None => IamApplicationSecret::new(prompt_secret("IAM Application secret: ")?)?,
            };
            let pairing = TestingEnvironmentIamPairing::new(
                *iam_environment_id,
                iam_environment_key,
                iam_app_id.clone(),
                iam_app_secret,
            );
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:pair-iam:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "replace-testing-environment-iam-pairing",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
                "pairing": pairing,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .replace_testing_environment_iam_pairing_with_key(*environment_id, &pairing, &key)
                .await?;
            // The previously saved test session belongs to the old IAM plane
            // and must never be presented after an atomic re-pair.
            credentials.remove_session(&profile, Some(*environment_id));
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            report_environment(output, &value);
            output.note(
                "the old test session was removed; sign in with an SLT from the new IAM plane",
            );
        }
        EnvCommand::Clean {
            environment_id: Some(environment_id),
        } => {
            let state = StateDirectory::locate()?;
            let credentials_lock = state.lock_credentials()?;
            let mut credentials = state.credentials()?;
            let scope = format!("env:clean:{profile}:{environment_id}");
            let fingerprint = request_fingerprint(&serde_json::json!({
                "operation": "clean-testing-environment",
                "profile": profile,
                "url": client.config().api_base().as_str(),
                "org": client.organization(),
                "environment_id": environment_id,
            }))?;
            let pending =
                prepare_pending_mutation(&state, &mut credentials, &scope, &fingerprint, None)?;
            let key = IdempotencyKey::new(pending.idempotency_key.clone())?;
            let value = client
                .clean_testing_environment_with_key(*environment_id, &key)
                .await?;
            credentials.clear_pending_mutation(&scope, &pending.idempotency_key);
            state.save_credentials(&credentials)?;
            drop(credentials_lock);
            report_cleaning(output, &value);
        }
        EnvCommand::Current
        | EnvCommand::Clean {
            environment_id: None,
        } => unreachable!(),
    }
    Ok(())
}

fn require_test(global: &GlobalArgs) -> Result<Uuid> {
    global
        .test
        .ok_or_else(|| CliError::usage("this action is only possible for a test environment"))
}

fn report_environment(output: Output, environment: &TestingEnvironment) {
    if output.is_json() {
        output.json(environment);
    } else {
        println!("id             {}", environment.id);
        println!("name           {}", environment.name);
        println!(
            "description    {}",
            environment.description.as_deref().unwrap_or("-")
        );
        println!("status         {:?}", environment.status);
        println!("key generation {}", environment.key_generation);
        println!("last activity  {}", environment.last_activity_at);
        println!("version        {}", environment.version);
    }
}

fn report_cleaning(output: Output, cleaning: &briefcase_client::TestingEnvironmentCleaning) {
    if output.is_json() {
        output.json(cleaning);
    } else {
        output.note(&format!("erased {} rows", cleaning.erased_rows));
    }
}

fn configure(global: &GlobalArgs, command: &ConfigCommand, output: Output) -> Result<()> {
    let state = StateDirectory::locate()?;
    // Writers lock before reading so concurrent setting changes merge instead
    // of replacing one another with snapshots taken at the same time.
    let _state_lock = match command {
        ConfigCommand::Set { .. } | ConfigCommand::Unset { .. } => Some(state.lock_credentials()?),
        ConfigCommand::Show => None,
    };
    let mut configuration = state.configuration()?;
    match command {
        ConfigCommand::Show => {
            if output.is_json() {
                output.json(&serde_json::json!({
                    "profile": global.profile.as_deref().unwrap_or(&configuration.current_profile),
                    "auto_update": configuration.auto_update,
                }));
            } else {
                println!(
                    "profile     {}",
                    global
                        .profile
                        .as_deref()
                        .unwrap_or(&configuration.current_profile)
                );
                println!(
                    "auto-update {}",
                    if configuration.auto_update {
                        "on"
                    } else {
                        "off"
                    }
                );
            }
        }
        ConfigCommand::Set { key, value } if key == "auto-update" => {
            configuration.auto_update = parse_switch(value)?;
            state.save_configuration(&configuration)?;
            output.note(if configuration.auto_update {
                "automatic CLI updates enabled"
            } else {
                "automatic CLI updates disabled"
            });
        }
        ConfigCommand::Unset { key } if key == "auto-update" => {
            configuration.auto_update = true;
            state.save_configuration(&configuration)?;
            output.note("automatic CLI updates restored to the default: on");
        }
        ConfigCommand::Set { key, .. } | ConfigCommand::Unset { key } => {
            return Err(CliError::usage(format!(
                "unknown setting `{key}`; supported setting: auto-update"
            )));
        }
    }
    Ok(())
}

fn parse_switch(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(CliError::usage("auto-update must be on or off")),
    }
}

async fn system(command: &SystemCommand, output: Output) -> Result<()> {
    match command {
        SystemCommand::Update => match crate::updater::update_now().await? {
            crate::updater::Outcome::Current(version) => {
                if output.is_json() {
                    output.json(&serde_json::json!({
                        "status": "current",
                        "version": version.to_string(),
                    }));
                } else {
                    output.note(&format!("briefcase {version} is current"));
                }
            }
            crate::updater::Outcome::Updated { from, to } => {
                if output.is_json() {
                    output.json(&serde_json::json!({
                        "status": "updated",
                        "from": from.to_string(),
                        "to": to.to_string(),
                    }));
                } else {
                    output.note(&format!("updated briefcase from {from} to {to}"));
                }
            }
            crate::updater::Outcome::Skipped => {}
        },
    }
    Ok(())
}

async fn version(global: &GlobalArgs, output: Output) -> Result<()> {
    let session = match session(global) {
        Ok(session) => Some(session),
        Err(error) if global.test.is_some() => return Err(error),
        Err(_) => None,
    };
    let client = match &session {
        Some(session) => Client::new_unchecked(config(session)?).ok(),
        None => None,
    };
    let served = match &client {
        Some(client) => client.version().await.ok(),
        None => None,
    };

    if output.is_json() {
        output.json(&serde_json::json!({
            "client": env!("CARGO_PKG_VERSION"),
            "client_api_version": briefcase_client::API_VERSION,
            "server": served.as_ref().map(|version| serde_json::json!({
                "build": version.build,
                "contract_version": version.contract_version,
                "supported_api_versions": version.supported_api_versions,
            })),
        }));
        return Ok(());
    }
    println!(
        "briefcase {} (contract {})",
        env!("CARGO_PKG_VERSION"),
        briefcase_client::API_VERSION
    );
    match served {
        Some(version) => println!(
            "server    build {} contract {} serving {}",
            version.build,
            version.contract_version,
            version.supported_api_versions.join(", ")
        ),
        None => println!("server    not reachable"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CliError, ensure_stored_scope, scope_for, split_path, testing_environment_for_login,
        unique_next_cursor,
    };
    use crate::state::{Credentials, Profile};
    use uuid::Uuid;

    #[test]
    fn a_path_splits_into_a_parent_and_a_name() {
        assert_eq!(
            split_path("public/handbook/onboarding"),
            (Some("public/handbook".to_owned()), "onboarding".to_owned())
        );
        assert_eq!(split_path("handbook"), (None, "handbook".to_owned()));
        assert_eq!(
            split_path("/private/cos:tos/notes/"),
            (Some("private/cos:tos".to_owned()), "notes".to_owned())
        );
    }

    #[test]
    fn exit_codes_tell_a_script_what_happened() {
        assert_eq!(CliError::usage("nope").exit_code(), 2);
        assert_eq!(
            CliError::Client(briefcase_client::Error::Configuration("x".into())).exit_code(),
            1
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

    #[test]
    fn stored_bearers_refuse_url_and_organization_overrides() {
        let mut credentials = Credentials::default();
        let bound = scope_for("https://briefcase.example/api/v1/", "tos").unwrap();
        credentials.set_credential_scope("work", None, bound);
        let saved = Profile {
            url: "https://briefcase.example/api/v1/".to_owned(),
            org: "tos".to_owned(),
        };

        let other_url = scope_for("https://attacker.example/api/v1/", "tos").unwrap();
        let error = ensure_stored_scope(&credentials, Some(&saved), "work", None, &other_url)
            .expect_err("a stored bearer must never follow a URL override");
        assert!(error.to_string().contains("refusing to send"));

        let other_org = scope_for("https://briefcase.example/api/v1/", "other").unwrap();
        let error = ensure_stored_scope(&credentials, Some(&saved), "work", None, &other_org)
            .expect_err("a stored bearer must never follow an organization override");
        assert!(error.to_string().contains("organization other"));
    }

    #[test]
    fn test_login_never_sends_a_stored_root_to_an_overridden_destination() {
        let environment_id = Uuid::from_u128(91);
        let root = "B2345678901234567890123456789012";
        let mut credentials = Credentials::default();
        credentials.set_testing_environment_key(
            "work",
            environment_id,
            briefcase_client::EnvironmentKey::new(root).unwrap(),
        );
        credentials.set_credential_scope(
            "work",
            Some(environment_id),
            scope_for("https://briefcase.example/api/v1/", "tos").unwrap(),
        );
        let saved = Profile {
            url: "https://briefcase.example/api/v1/".to_owned(),
            org: "tos".to_owned(),
        };
        let overridden = scope_for("https://attacker.example/api/v1/", "tos").unwrap();

        let error = testing_environment_for_login(
            &credentials,
            Some(&saved),
            "work",
            environment_id,
            &overridden,
        )
        .expect_err("test login must reject the override before constructing a client");
        assert!(error.to_string().contains("refusing to send"));
        assert!(!error.to_string().contains(root));
    }

    #[test]
    fn legacy_credentials_inherit_only_their_existing_profile_scope() {
        let environment_id = Uuid::from_u128(92);
        let mut credentials = Credentials::default();
        credentials.set_testing_environment_key(
            "work",
            environment_id,
            briefcase_client::EnvironmentKey::new("C2345678901234567890123456789012").unwrap(),
        );
        let saved = Profile {
            url: "https://briefcase.example/api/v1/".to_owned(),
            org: "tos".to_owned(),
        };
        let original = scope_for(&saved.url, &saved.org).unwrap();
        assert!(
            testing_environment_for_login(
                &credentials,
                Some(&saved),
                "work",
                environment_id,
                &original,
            )
            .is_ok()
        );

        let overridden = scope_for("https://attacker.example/api/v1/", "tos").unwrap();
        assert!(
            testing_environment_for_login(
                &credentials,
                Some(&saved),
                "work",
                environment_id,
                &overridden,
            )
            .is_err()
        );
    }
}
