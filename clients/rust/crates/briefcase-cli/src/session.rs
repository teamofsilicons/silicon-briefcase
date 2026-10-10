//! The saved sign-in, refreshed single-flight and turned into a connected client.

use briefcase_client::{
    Client, Config,
    accounts::{SignIn, SignInError, SignedIn},
};
use secrecy::ExposeSecret as _;
use time::{Duration, OffsetDateTime};

use crate::{
    cli::GlobalArgs,
    error::{CliError, Result, SIGN_IN, SIGN_IN_AGAIN},
    state::{Configuration, Credentials, StateDirectory, StoredAccount, StoredSession},
};

/// The hosted Briefcase API.
pub const DEFAULT_API_URL: &str = briefcase_client::DEFAULT_API_URL;
/// The Silicon Accounts deployment Briefcase signs in at.
pub const DEFAULT_ACCOUNTS_URL: &str = briefcase_client::accounts::DEFAULT_ACCOUNTS_URL;
/// Keep using a token that cannot be refreshed right now only while it has this long left.
const GRACE: Duration = Duration::seconds(10);

/// Which profile and deployment a command uses.
pub struct Selection {
    /// The state directory.
    pub state: StateDirectory,
    /// The profile in use.
    pub profile: String,
    /// The Briefcase API URL in use (with a trailing slash).
    pub api_url: String,
    /// The Silicon Accounts URL in use (without a trailing slash).
    pub accounts_url: String,
}

/// Ensures exactly one trailing slash on an API base.
#[must_use]
pub fn normalize_api_url(url: &str) -> String {
    format!("{}/", url.trim().trim_end_matches('/'))
}

/// Removes trailing slashes from a Silicon Accounts URL.
#[must_use]
pub fn normalize_accounts_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

/// Resolves the profile and the URLs: options and environment first, then the
/// saved profile, then the hosted defaults.
///
/// # Errors
///
/// Returns a state error when the state directory or its configuration cannot be read.
pub fn select(global: &GlobalArgs) -> Result<Selection> {
    let state = StateDirectory::locate()?;
    let configuration = state.configuration()?;
    Ok(selection_from(global, state, &configuration))
}

/// Like [`select`], with already-loaded state.
pub fn selection_from(
    global: &GlobalArgs,
    state: StateDirectory,
    configuration: &Configuration,
) -> Selection {
    let profile = global
        .profile
        .clone()
        .unwrap_or_else(|| configuration.current_profile.clone());
    let saved = configuration.profiles.get(&profile);
    let api_url = normalize_api_url(
        global
            .url
            .as_deref()
            .or_else(|| saved.map(|profile| profile.url.as_str()))
            .unwrap_or(DEFAULT_API_URL),
    );
    let accounts_url = normalize_accounts_url(
        global
            .accounts_url
            .as_deref()
            .or_else(|| saved.and_then(|profile| profile.accounts_url.as_deref()))
            .unwrap_or(DEFAULT_ACCOUNTS_URL),
    );
    Selection {
        state,
        profile,
        api_url,
        accounts_url,
    }
}

/// A connected client and who it acts as.
pub struct Context {
    /// The Briefcase client, carrying the access token.
    pub client: Client,
    /// The profile in use.
    pub profile: String,
    /// The signed-in account; learned from Briefcase on first use with `--token`.
    account: Option<StoredAccount>,
}

impl Context {
    /// The signed-in account, when known without asking Briefcase.
    #[must_use]
    pub const fn account(&self) -> Option<&StoredAccount> {
        self.account.as_ref()
    }

    /// The signed-in account's uuid: the key of its own drive.
    ///
    /// # Errors
    ///
    /// Returns Briefcase's refusal when the token does not verify.
    pub async fn own_uuid(&mut self) -> Result<String> {
        if let Some(account) = &self.account {
            return Ok(account.uuid.clone());
        }
        let me = self.client.me().await?;
        let account = StoredAccount {
            uuid: me.uuid.clone(),
            id: me.id.clone().unwrap_or_default(),
            kind: me.kind,
            display_name: me.display_name.clone(),
            custodian: me.custodian,
        };
        self.account = Some(account);
        Ok(me.uuid)
    }
}

/// Connects with the saved sign-in (refreshed when needed) or `--token`.
///
/// # Errors
///
/// Returns `not_signed_in` (or why the sign-in ended), a refusal to send a
/// sign-in to another deployment than its own, a refresh failure, or a
/// contract mismatch.
pub async fn connect(global: &GlobalArgs) -> Result<Context> {
    let selection = select(global)?;
    if let Some(token) = global
        .token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
    {
        let client = client(global, &selection.api_url, Some(token)).await?;
        return Ok(Context {
            client,
            profile: selection.profile,
            account: None,
        });
    }
    let session = usable_session(&selection, global).await?;
    let client = client(global, &session.api_url, Some(&session.access_token)).await?;
    Ok(Context {
        client,
        profile: selection.profile,
        account: Some(session.account),
    })
}

/// Builds a client for `api_url`, presenting `token` when given, checking the
/// contract first unless `--no-verify`.
///
/// # Errors
///
/// Returns a configuration error for an unusable URL, a transport error, or an
/// incompatible contract.
pub async fn client(global: &GlobalArgs, api_url: &str, token: Option<&str>) -> Result<Client> {
    let mut config = Config::new(api_url)?
        .with_telemetry(crate::telemetry::enabled())
        .with_telemetry_source(briefcase_client::telemetry::Source::Cli)
        .with_user_agent(concat!("briefcase-cli/", env!("CARGO_PKG_VERSION")));
    if let Some(token) = token {
        config = config.with_token(token);
    }
    Ok(if global.no_verify {
        Client::new_unchecked(config)?
    } else {
        Client::connect(config).await?
    })
}

/// The profile's sign-in, refreshed single-flight when less than a minute is left.
///
/// # Errors
///
/// As [`connect`].
pub async fn usable_session(selection: &Selection, global: &GlobalArgs) -> Result<StoredSession> {
    let credentials = selection.state.credentials()?;
    let Some(session) = credentials.session(&selection.profile).cloned() else {
        return Err(not_signed_in(&selection.profile, &credentials));
    };
    check_binding(global, &selection.profile, &session)?;
    if !session.needs_refresh() {
        return Ok(session);
    }
    refresh(&selection.state, &selection.profile, false).await
}

/// Refuses to send a saved sign-in to another API than the one it was made for.
///
/// # Errors
///
/// Returns `sign_in_bound_elsewhere` (exit code 2).
pub fn check_binding(global: &GlobalArgs, profile: &str, session: &StoredSession) -> Result<()> {
    if let Some(url) = &global.url
        && normalize_api_url(url) != normalize_api_url(&session.api_url)
    {
        return Err(CliError::refused(
            "sign_in_bound_elsewhere",
            format!(
                "The saved sign-in of profile {profile} is for {}; it is never sent to {}.",
                session.api_url,
                normalize_api_url(url)
            ),
            Some(
                "Sign in for that deployment under its own profile (`briefcase --url <url> login --save-as <name>`), or drop --url."
                    .to_owned(),
            ),
            2,
        ));
    }
    Ok(())
}

/// Refreshes a profile's sign-in under the credentials lock.
///
/// The file is read again once the lock is held: when another process
/// refreshed meanwhile, its pair is used and nothing is sent. Otherwise the
/// refresh token is presented once, and the new pair is saved before it is
/// used. A refused refresh token signs the profile out (and remembers why); a
/// Silicon Accounts that cannot be reached leaves the pair untouched and, while
/// the access token still has ten seconds, uses it.
///
/// # Errors
///
/// Returns `sign_in_ended`, `accounts_unreachable` or a state error.
pub async fn refresh(state: &StateDirectory, profile: &str, force: bool) -> Result<StoredSession> {
    let _lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    let Some(current) = credentials.session(profile).cloned() else {
        return Err(not_signed_in(profile, &credentials));
    };
    if !force && !current.needs_refresh() {
        return Ok(current);
    }
    let sign_in = SignIn::for_app(&current.accounts_url, &current.app_id)?;
    let started = OffsetDateTime::now_utc();
    match sign_in.refresh(&current.refresh_token).await {
        Ok(signed_in) => {
            let next = refreshed(&current, &signed_in, started)?;
            credentials.set_session(profile, next.clone());
            state.save_credentials(&credentials)?;
            Ok(next)
        }
        Err(SignInError::SignInEnded { description }) => {
            credentials.end_session(profile, "sign_in_ended", &description);
            state.save_credentials(&credentials)?;
            Err(CliError::SignIn(SignInError::SignInEnded { description }))
        }
        Err(error) if error.is_transient() && current.usable_for(GRACE) => {
            eprintln!(
                "briefcase: the sign-in could not be refreshed ({error}); its access token still works until {}",
                crate::render::timestamp(current.expires_at)
            );
            Ok(current)
        }
        Err(error) => Err(error.into()),
    }
}

/// The session after a refresh: the new pair, and the account as Silicon
/// Accounts describes it now.
fn refreshed(
    current: &StoredSession,
    signed_in: &SignedIn,
    started: OffsetDateTime,
) -> Result<StoredSession> {
    if signed_in.account.uuid != current.account.uuid {
        return Err(CliError::refused(
            "account_changed",
            format!(
                "Silicon Accounts refreshed the sign-in of {} as another account ({}); nothing was saved.",
                current.account.display_id(),
                signed_in.account.display_id()
            ),
            Some(SIGN_IN_AGAIN.to_owned()),
            4,
        ));
    }
    let refresh_token = signed_in
        .refresh_token
        .as_ref()
        .map(|token| token.expose_secret().to_owned())
        .ok_or_else(|| {
            CliError::refused(
                "refresh_token_missing",
                "Silicon Accounts refreshed the sign-in without a new refresh token.",
                Some(SIGN_IN_AGAIN.to_owned()),
                4,
            )
        })?;
    Ok(StoredSession {
        access_token: signed_in.access_token.expose_secret().to_owned(),
        refresh_token,
        expires_at: started
            + Duration::seconds(i64::try_from(signed_in.expires_in.as_secs()).unwrap_or(i64::MAX)),
        refresh_expires_at: signed_in.refresh_expires_at.or(current.refresh_expires_at),
        scope: if signed_in.scope.is_empty() {
            current.scope.clone()
        } else {
            signed_in.scope.clone()
        },
        account: stored_account(&signed_in.account),
        ..current.clone()
    })
}

/// The stored form of a signed-in account.
#[must_use]
pub fn stored_account(account: &briefcase_client::accounts::SignedInAccount) -> StoredAccount {
    StoredAccount {
        uuid: account.uuid.clone(),
        id: account.id.clone(),
        kind: account.kind,
        display_name: account.display_name.clone(),
        custodian: account.custodian.clone(),
    }
}

/// Why a profile has no usable sign-in.
#[must_use]
pub fn not_signed_in(profile: &str, credentials: &Credentials) -> CliError {
    if let Some(id) = credentials
        .legacy
        .as_ref()
        .and_then(|legacy| legacy.profiles.get(profile))
    {
        return CliError::refused(
            "signed_in_before_silicon_accounts",
            format!(
                "Profile {profile} was signed in{} before Briefcase moved to Silicon Accounts; that sign-in no longer works.",
                id.as_deref()
                    .map(|id| format!(" as {id}"))
                    .unwrap_or_default()
            ),
            Some(SIGN_IN_AGAIN.to_owned()),
            4,
        );
    }
    if let Some(ended) = credentials.ended.get(profile) {
        return CliError::refused(
            ended.reason.clone(),
            format!(
                "The sign-in of profile {profile}{} ended: {}",
                ended
                    .account_id
                    .as_deref()
                    .map(|id| format!(" (as {id})"))
                    .unwrap_or_default(),
                ended.message
            ),
            Some(SIGN_IN_AGAIN.to_owned()),
            4,
        );
    }
    CliError::refused(
        "not_signed_in",
        format!("Profile {profile} is not signed in to Briefcase."),
        Some(SIGN_IN.to_owned()),
        4,
    )
}
