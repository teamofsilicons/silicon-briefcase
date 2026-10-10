//! Signing in and out at Silicon Accounts, and saying who is signed in.

use briefcase_client::{
    Client, Config,
    accounts::{
        APP_ID, DeviceEvent, DeviceRequest, DeviceSignIn, MINT_COMMAND, SignIn, SignInError,
        SignedIn,
    },
};
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

use crate::{
    cli::{GlobalArgs, LoginArgs},
    error::{CliError, Result, SIGN_IN, SIGN_IN_AGAIN},
    input::read_secret_stdin,
    render::{Output, timestamp},
    session::{
        self, DEFAULT_ACCOUNTS_URL, DEFAULT_API_URL, Selection, normalize_accounts_url,
        normalize_api_url,
    },
    state::{
        Configuration, Credentials, Profile, SignInMethod, StateDirectory, StateError,
        StoredSession,
    },
};

/// The website Carbons use.
pub const WEB_URL: &str = "https://briefcase.teamofsilicons.com";
/// How a Silicon signs in, in one line.
pub const SILICON_SIGN_IN: &str =
    "silicon-accounts login --app briefcase -q | briefcase login --slt-stdin";

/// `briefcase login`: a device code for Carbons, a short-lived token for Silicons.
///
/// # Errors
///
/// Returns the exact refusal: a deployment that does not match, a refused
/// short-lived token (with its reason), a denied or expired device code, or a
/// state error.
pub async fn login(global: &GlobalArgs, args: &LoginArgs, output: Output) -> Result<()> {
    if global.token.is_some() {
        return Err(CliError::usage(
            "--token (or BRIEFCASE_TOKEN) is an access-token override for scripts, not a way to sign in; run `briefcase login` without it",
        ));
    }
    // Read the token before anything else, so a prompt never waits on the network.
    let slt = if args.slt_stdin {
        Some(read_secret_stdin()?)
    } else {
        args.slt_positional.clone().or_else(|| args.slt.clone())
    };
    if slt.as_deref().is_some_and(|slt| slt.trim().is_empty()) {
        return Err(CliError::usage(format!(
            "the short-lived token was empty; mint one with `{MINT_COMMAND}` and pipe it to `briefcase login --slt-stdin`"
        )));
    }
    let mut global = global.clone();
    if let Some(name) = &args.save_as {
        if name.trim().is_empty() {
            return Err(CliError::usage("--save-as needs a profile name"));
        }
        global.profile = Some(name.trim().to_owned());
    }
    let state = StateDirectory::locate()?;
    let configuration = readable_configuration(&state)?;
    let selection = session::selection_from(&global, state, &configuration);

    // A single-use code or token is never spent on a deployment that would refuse it.
    check_deployment(&selection).await?;
    let sign_in = SignIn::new(&selection.accounts_url)?;
    let (signed_in, method) = match slt {
        Some(slt) => (
            sign_in.exchange_short_lived_token(&slt).await?,
            SignInMethod::ShortLivedToken,
        ),
        None => (
            device_sign_in(&sign_in, args, output).await?,
            SignInMethod::Device,
        ),
    };
    let session = save(&selection, &signed_in, method).await?;
    let result = signed_in_json(&session, &selection.profile, true);
    if output.is_json() {
        output.json_line(&result);
    } else {
        output.note(&format!(
            "Signed in to Briefcase as {}{}, a {}, on profile {}.\nThe sign-in is saved in {} and refreshes itself; `briefcase logout` ends it.",
            session.account.display_id(),
            session
                .account
                .display_name
                .as_deref()
                .map(|name| format!(" ({name})"))
                .unwrap_or_default(),
            if session.account.kind == briefcase_client::AccountKind::Silicon {
                "Silicon"
            } else {
                "Carbon"
            },
            selection.profile,
            selection.state.credentials_path().display()
        ));
    }
    Ok(())
}

/// Reads the configuration; an unreadable one is moved aside so signing in can
/// start fresh (nothing is deleted).
fn readable_configuration(state: &StateDirectory) -> Result<Configuration> {
    match state.configuration() {
        Ok(configuration) => Ok(configuration),
        Err(StateError::Malformed { path, reason }) => {
            let _lock = state.lock_credentials()?;
            let aside = state.set_aside_unreadable(StateDirectory::CONFIG)?;
            eprintln!(
                "briefcase: {path} was not readable ({reason}); kept as {} and started fresh",
                aside.map_or_else(|| path.clone(), |aside| aside.display().to_string())
            );
            Ok(Configuration::default())
        }
        Err(error) => Err(error.into()),
    }
}

/// Asks the deployment which app and Silicon Accounts it trusts, and refuses a
/// mismatch before a single-use code or token is spent. A deployment that
/// cannot be reached only warns: signing in does not need it.
async fn check_deployment(selection: &Selection) -> Result<()> {
    let client = Client::new_unchecked(Config::new(&selection.api_url)?.with_telemetry(false))?;
    match client.accounts_info().await {
        Ok(info) => {
            if info.app_id != APP_ID {
                return Err(CliError::refused(
                    "app_id_mismatch",
                    format!(
                        "{} answers for the app `{}`, not `{APP_ID}`; nothing was sent to Silicon Accounts.",
                        selection.api_url, info.app_id
                    ),
                    Some("Check --url / BRIEFCASE_URL.".to_owned()),
                    2,
                ));
            }
            if normalize_accounts_url(&info.accounts_url) != selection.accounts_url {
                return Err(CliError::refused(
                    "accounts_mismatch",
                    format!(
                        "{} accepts sign-ins from {}, but this command would sign in at {}; nothing was spent.",
                        selection.api_url, info.accounts_url, selection.accounts_url
                    ),
                    Some(format!(
                        "Set ACCOUNTS_URL={} (or --accounts-url), or point --url at the Briefcase that uses {}.",
                        info.accounts_url, selection.accounts_url
                    )),
                    2,
                ));
            }
            Ok(())
        }
        Err(error) if matches!(error.status(), Some(404 | 406)) => Err(CliError::refused(
            "service_not_accounts",
            format!(
                "{} does not sign in with Silicon Accounts (it answered {} for /api/v1/accounts): it runs a Briefcase older than 4.0.",
                selection.api_url,
                error.status().unwrap_or_default()
            ),
            Some("Point --url at a Briefcase 4 deployment.".to_owned()),
            2,
        )),
        Err(error) => {
            eprintln!(
                "briefcase: could not check {} before signing in ({error}); signing in at {} anyway",
                selection.api_url, selection.accounts_url
            );
            Ok(())
        }
    }
}

/// The device flow: show the code, optionally open the page, wait for approval.
async fn device_sign_in(sign_in: &SignIn, args: &LoginArgs, output: Output) -> Result<SignedIn> {
    let label = args.label.clone().unwrap_or_else(default_label);
    let asked = args.scope.clone();
    let request = DeviceRequest {
        // Invitation emails reach a Carbon only when it shares its email.
        scope: asked.clone().or_else(|| Some("email".to_owned())),
        label: Some(label.clone()),
    };
    let device = match sign_in.start_device_sign_in(&request).await {
        // Briefcase's setup may not ask for email; then sign in without it.
        Err(SignInError::Accounts { code, .. }) if code == "invalid_scope" && asked.is_none() => {
            sign_in
                .start_device_sign_in(&DeviceRequest {
                    scope: None,
                    label: Some(label),
                })
                .await?
        }
        other => other?,
    };
    announce(&device, output);
    if args.open {
        open_in_browser(device.browser_url());
    }
    Ok(sign_in
        .finish_device_sign_in(&device, |event| report(&event, output))
        .await?)
}

fn announce(device: &DeviceSignIn, output: Output) {
    if output.is_json() {
        output.json_line(&json!({
            "event": "device_code",
            "user_code": device.user_code,
            "verification_uri": device.verification_uri,
            "verification_uri_complete": device.verification_uri_complete,
            "expires_in": device.expires_in.as_secs(),
            "interval": device.interval.as_secs(),
        }));
        return;
    }
    eprintln!(
        "To sign in to Briefcase, open this page and confirm the code:\n\n  {}\n  Code: {}\n\n{}Waiting for approval; the code expires in {} minutes.",
        device.verification_uri,
        device.user_code,
        device
            .verification_uri_complete
            .as_deref()
            .map(|complete| format!("(or open {complete})\n"))
            .unwrap_or_default(),
        device.expires_in.as_secs().div_ceil(60)
    );
}

fn report(event: &DeviceEvent, output: Output) {
    match event {
        DeviceEvent::Pending => {}
        DeviceEvent::SlowDown { interval } => {
            if output.is_json() {
                output.json_line(&json!({"event": "slow_down", "interval": interval.as_secs()}));
            }
        }
        DeviceEvent::Retrying { reason, retry_in } => {
            if output.is_json() {
                output.json_line(&json!({
                    "event": "retrying", "reason": reason, "retry_in": retry_in.as_secs()
                }));
            } else {
                eprintln!(
                    "briefcase: {reason}; asking again in {} seconds",
                    retry_in.as_secs()
                );
            }
        }
    }
}

/// `briefcase CLI on <host>`, from the environment or `/etc/hostname` (no subprocess).
fn default_label() -> String {
    let host = ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|host| host.trim().to_owned())
        .filter(|host| !host.is_empty());
    host.map_or_else(
        || "briefcase CLI".to_owned(),
        |host| format!("briefcase CLI on {host}"),
    )
}

fn open_in_browser(url: &str) {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    let opened = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = opened {
        eprintln!("briefcase: could not open a browser ({error}); open the page above yourself");
    }
}

/// Saves a new sign-in for the profile under the lock, makes the profile
/// current, and revokes the sign-in it replaces (best effort) so that one can
/// never be used again from this home.
async fn save(
    selection: &Selection,
    signed_in: &SignedIn,
    method: SignInMethod,
) -> Result<StoredSession> {
    let now = OffsetDateTime::now_utc();
    let refresh_token = signed_in
        .refresh_token
        .as_ref()
        .map(|token| token.expose_secret().to_owned())
        .ok_or_else(|| {
            CliError::refused(
                "refresh_token_missing",
                "Silicon Accounts signed the account in without a refresh token, so the sign-in could not be kept.",
                Some(SIGN_IN_AGAIN.to_owned()),
                4,
            )
        })?;
    let session = StoredSession {
        api_url: selection.api_url.clone(),
        accounts_url: selection.accounts_url.clone(),
        app_id: APP_ID.to_owned(),
        account: session::stored_account(&signed_in.account),
        access_token: signed_in.access_token.expose_secret().to_owned(),
        refresh_token,
        expires_at: now
            + Duration::seconds(i64::try_from(signed_in.expires_in.as_secs()).unwrap_or(i64::MAX)),
        refresh_expires_at: signed_in.refresh_expires_at,
        scope: signed_in.scope.clone(),
        signed_in_at: now,
        method,
    };
    let state = &selection.state;
    let lock = state.lock_credentials()?;
    let mut credentials = match state.credentials() {
        Ok(credentials) => credentials,
        Err(StateError::Malformed { path, reason }) => {
            let aside = state.set_aside_unreadable(StateDirectory::CREDENTIALS)?;
            eprintln!(
                "briefcase: {path} was not readable ({reason}); kept as {} and started fresh",
                aside.map_or_else(|| path.clone(), |aside| aside.display().to_string())
            );
            Credentials::default()
        }
        Err(error) => return Err(error.into()),
    };
    let replaced = credentials.session(&selection.profile).cloned();
    credentials.set_session(&selection.profile, session.clone());
    state.save_credentials(&credentials)?;
    let mut configuration = state.configuration().unwrap_or_default();
    configuration.profiles.insert(
        selection.profile.clone(),
        Profile {
            url: selection.api_url.clone(),
            accounts_url: Some(selection.accounts_url.clone()),
        },
    );
    configuration.current_profile.clone_from(&selection.profile);
    state.save_configuration(&configuration)?;
    drop(lock);
    if let Some(replaced) = replaced
        && replaced.refresh_token != session.refresh_token
        && let Ok(sign_in) = SignIn::for_app(&replaced.accounts_url, &replaced.app_id)
        && let Err(error) = sign_in.revoke(&replaced.refresh_token).await
    {
        eprintln!(
            "briefcase: the sign-in this one replaced (as {}) could not be revoked ({error}); it is forgotten here",
            replaced.account.display_id()
        );
    }
    Ok(session)
}

/// The signed-in answer of `login` and `login status`.
fn signed_in_json(session: &StoredSession, profile: &str, verified: bool) -> Value {
    let mut value = json!({
        "authenticated": true,
        "uuid": session.account.uuid,
        "id": session.account.id,
        "kind": session.account.kind,
        "expires_at": rfc3339(session.expires_at),
        "refresh_expires_at": session.refresh_expires_at.map(rfc3339),
        "verified": verified,
        "profile": profile,
        "api_url": session.api_url,
        "accounts_url": session.accounts_url,
        "method": session.method,
        "signed_in_at": rfc3339(session.signed_in_at),
    });
    if let Some(name) = &session.account.display_name {
        value["display_name"] = name.clone().into();
    }
    if let Some(custodian) = &session.account.custodian {
        value["custodian"] = json!({"uuid": custodian.uuid, "id": custodian.id});
    }
    value
}

fn rfc3339(moment: OffsetDateTime) -> String {
    moment
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| moment.unix_timestamp().to_string())
}

/// `briefcase logout`: revoke this profile's refresh token at Silicon Accounts
/// and forget the sign-in, even when Silicon Accounts cannot be reached.
///
/// # Errors
///
/// Returns a state error when the sign-ins cannot be read or written.
pub async fn logout(global: &GlobalArgs, output: Output) -> Result<()> {
    let selection = session::select(global)?;
    let state = &selection.state;
    let lock = state.lock_credentials()?;
    let mut credentials = state.credentials()?;
    let had_ended = credentials.ended.remove(&selection.profile).is_some();
    let Some(signed_in) = credentials.remove_session(&selection.profile) else {
        if had_ended {
            state.save_credentials(&credentials)?;
        }
        drop(lock);
        if output.is_json() {
            output.json(&json!({"signed_out": false, "reason": "not_signed_in", "profile": selection.profile}));
        } else {
            output.note(&format!("Profile {} was not signed in.", selection.profile));
        }
        return Ok(());
    };
    // Revoke while holding the lock, so no refresh rotates the token meanwhile.
    let revocation = match SignIn::for_app(&signed_in.accounts_url, &signed_in.app_id) {
        Ok(sign_in) => sign_in
            .revoke(&signed_in.refresh_token)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    state.save_credentials(&credentials)?;
    drop(lock);
    let (revoked, warning) = match revocation {
        Ok(revocation) => (revocation.revoked, revocation.message),
        Err(error) => (
            false,
            Some(format!(
                "the refresh token could not be revoked at Silicon Accounts ({error}); it is forgotten here and stops working when it expires, or when the account signs out of Briefcase at Silicon Accounts"
            )),
        ),
    };
    if output.is_json() {
        let mut value = json!({
            "signed_out": true,
            "revoked": revoked,
            "profile": selection.profile,
            "account": {"uuid": signed_in.account.uuid, "id": signed_in.account.id},
        });
        if let Some(warning) = &warning {
            value["warning"] = warning.clone().into();
        }
        output.json(&value);
    } else {
        output.note(&format!(
            "Signed {} out of Briefcase on profile {}{}.",
            signed_in.account.display_id(),
            selection.profile,
            if revoked {
                "; the sign-in is revoked"
            } else {
                ""
            }
        ));
        if let Some(warning) = warning {
            eprintln!("briefcase: {warning}");
        }
    }
    Ok(())
}

/// `briefcase login status`: whether this profile is signed in, and as whom.
/// With `--json` it always exits 0; without, 1 when signed out.
///
/// # Errors
///
/// Only [`CliError::Exit`]-style refusals for the human signed-out case.
pub async fn login_status(global: &GlobalArgs, offline: bool, output: Output) -> Result<()> {
    let report = status_report(global, offline).await;
    let authenticated = report["authenticated"].as_bool().unwrap_or(false);
    if output.is_json() {
        output.json(&report);
        return Ok(());
    }
    if authenticated {
        println!(
            "Signed in as {}{}, a {}.",
            report["id"].as_str().unwrap_or_default(),
            report["display_name"]
                .as_str()
                .map(|name| format!(" ({name})"))
                .unwrap_or_default(),
            report["kind"].as_str().unwrap_or("carbon")
        );
        for (label, key) in [
            ("uuid", "uuid"),
            ("profile", "profile"),
            ("briefcase", "api_url"),
            ("accounts", "accounts_url"),
            ("expires", "expires_at"),
        ] {
            if let Some(value) = report[key].as_str() {
                println!("  {label:<10} {value}");
            }
        }
        println!(
            "  {:<10} {}",
            "verified",
            if report["verified"].as_bool().unwrap_or(false) {
                "yes, by Briefcase just now"
            } else if offline {
                "no (--offline: read from the saved sign-in only)"
            } else {
                "no"
            }
        );
        if let Some(warning) = report["warning"].as_str() {
            eprintln!("briefcase: {warning}");
        }
        return Ok(());
    }
    println!(
        "Not signed in.{}",
        report["message"]
            .as_str()
            .map(|message| format!(" {message}"))
            .unwrap_or_default()
    );
    println!("{}", report["hint"].as_str().unwrap_or(SIGN_IN));
    Err(CliError::Exit(1))
}

/// The answer of `login status --json`.
async fn status_report(global: &GlobalArgs, offline: bool) -> Value {
    if let Some(token) = global
        .token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
    {
        return token_report(global, token, offline).await;
    }
    let Ok(state) = StateDirectory::locate() else {
        // No home at all: nothing can be signed in.
        return json!({"authenticated": false});
    };
    let configuration = match state.configuration() {
        Ok(configuration) => configuration,
        Err(error) => return unreadable(&error),
    };
    let credentials = match state.credentials() {
        Ok(credentials) => credentials,
        Err(error) => return unreadable(&error),
    };
    let selection = session::selection_from(global, state, &configuration);
    let Some(saved) = credentials.session(&selection.profile).cloned() else {
        return match session::not_signed_in(&selection.profile, &credentials) {
            CliError::Refused { code, .. } if code == "not_signed_in" => {
                json!({"authenticated": false})
            }
            refusal => signed_out(&refusal),
        };
    };
    if offline {
        return signed_in_json(&saved, &selection.profile, false);
    }
    if let Err(refusal) = session::check_binding(global, &selection.profile, &saved) {
        let mut value = signed_in_json(&saved, &selection.profile, false);
        value["warning"] = refusal.to_string().into();
        return value;
    }
    verify(global, &selection, saved).await
}

/// Verifies a saved sign-in with Briefcase (`GET /api/v1/me`), refreshing it
/// first when needed and once more when Briefcase refuses the access token.
async fn verify(global: &GlobalArgs, selection: &Selection, saved: StoredSession) -> Value {
    let profile = &selection.profile;
    let mut current = saved.clone();
    for attempt in 0..2 {
        let refreshed = if attempt == 0 && !current.needs_refresh() {
            Ok(current.clone())
        } else {
            session::refresh(&selection.state, profile, attempt > 0).await
        };
        current = match refreshed {
            Ok(session) => session,
            Err(error @ CliError::SignIn(SignInError::SignInEnded { .. })) => {
                return signed_out(&error);
            }
            Err(error) => {
                let mut value = signed_in_json(&current, profile, false);
                value["warning"] = format!("the sign-in could not be refreshed: {error}").into();
                return value;
            }
        };
        let client =
            match session::client(global, &current.api_url, Some(&current.access_token)).await {
                Ok(client) => client,
                Err(error) => {
                    let mut value = signed_in_json(&current, profile, false);
                    value["warning"] =
                        format!("Briefcase could not verify the sign-in: {error}").into();
                    return value;
                }
            };
        match client.me().await {
            Ok(me) => {
                let mut value = signed_in_json(&current, profile, true);
                if let Some(id) = &me.id {
                    value["id"] = id.clone().into();
                }
                if let Some(name) = &me.display_name {
                    value["display_name"] = name.clone().into();
                }
                if let Some(custodian) = &me.custodian {
                    value["custodian"] = json!({"uuid": custodian.uuid, "id": custodian.id});
                }
                remember_identity(&selection.state, profile, &me);
                return value;
            }
            Err(error) if error.is_unauthenticated() && attempt == 0 => {}
            Err(error) if error.is_unauthenticated() => {
                return signed_out(&CliError::Client(error));
            }
            Err(error) => {
                let mut value = signed_in_json(&current, profile, false);
                value["warning"] =
                    format!("Briefcase could not verify the sign-in: {error}").into();
                return value;
            }
        }
    }
    signed_in_json(&current, profile, false)
}

/// Keeps the saved account's id and name current (ids can change), best effort.
fn remember_identity(state: &StateDirectory, profile: &str, me: &briefcase_client::Account) {
    let Ok(_lock) = state.lock_credentials() else {
        return;
    };
    let Ok(mut credentials) = state.credentials() else {
        return;
    };
    let Some(saved) = credentials.sessions.get_mut(profile) else {
        return;
    };
    if saved.account.uuid != me.uuid {
        return;
    }
    let id = me.id.clone().unwrap_or_else(|| saved.account.id.clone());
    if saved.account.id == id && saved.account.display_name == me.display_name {
        return;
    }
    saved.account.id = id;
    saved.account.display_name.clone_from(&me.display_name);
    saved.account.custodian.clone_from(&me.custodian);
    let _ = state.save_credentials(&credentials);
}

async fn token_report(global: &GlobalArgs, token: &str, offline: bool) -> Value {
    let claims = token_claims(token);
    let unverified = |warning: Option<String>| {
        let mut value = json!({
            "authenticated": true,
            "uuid": claims.get("sub"),
            "id": claims.get("id"),
            "kind": claims.get("kind"),
            "expires_at": claims.get("exp").and_then(Value::as_i64)
                .and_then(|exp| OffsetDateTime::from_unix_timestamp(exp).ok()).map(rfc3339),
            "verified": false,
            "source": "token",
        });
        if let Some(warning) = warning {
            value["warning"] = warning.into();
        }
        value
    };
    if offline {
        return unverified(None);
    }
    let api_url = global.url.as_deref().map_or_else(
        || {
            session::select(global).map_or_else(
                |_| DEFAULT_API_URL.to_owned(),
                |selection| selection.api_url,
            )
        },
        normalize_api_url,
    );
    let client = match session::client(global, &api_url, Some(token)).await {
        Ok(client) => client,
        Err(error) => {
            return unverified(Some(format!(
                "Briefcase could not verify the token: {error}"
            )));
        }
    };
    match client.me().await {
        Ok(me) => {
            let mut value = json!({
                "authenticated": true,
                "uuid": me.uuid,
                "id": me.id,
                "kind": me.kind,
                "expires_at": unverified(None)["expires_at"].clone(),
                "verified": true,
                "source": "token",
            });
            if let Some(name) = me.display_name {
                value["display_name"] = name.into();
            }
            value
        }
        Err(error) if error.is_unauthenticated() => signed_out(&CliError::Client(error)),
        Err(error) => unverified(Some(format!(
            "Briefcase could not verify the token: {error}"
        ))),
    }
}

/// Reads a JWT's claims without verifying them (display only).
fn token_claims(token: &str) -> serde_json::Map<String, Value> {
    use base64::Engine as _;
    token
        .split('.')
        .nth(1)
        .and_then(|payload| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn signed_out(refusal: &CliError) -> Value {
    let mut value = json!({
        "authenticated": false,
        "reason": refusal.code(),
        "message": refusal.to_string(),
    });
    if let Some(hint) = refusal.hint() {
        value["hint"] = hint.into();
    }
    value
}

fn unreadable(error: &StateError) -> Value {
    signed_out(&CliError::refused(
        "unreadable_state",
        error.to_string(),
        Some(
            "Run `briefcase login` to sign in again; the unreadable file is kept beside it."
                .to_owned(),
        ),
        4,
    ))
}

/// `briefcase accounts`: how Briefcase signs accounts in. Offline, never fails.
pub fn accounts(global: &GlobalArgs, output: Output) {
    // The saved profile's URLs when there is one; state that cannot be read is ignored.
    let saved = StateDirectory::locate()
        .and_then(|state| state.configuration())
        .ok()
        .and_then(|configuration| {
            let profile = global
                .profile
                .clone()
                .unwrap_or_else(|| configuration.current_profile.clone());
            configuration.profiles.get(&profile).cloned()
        });
    let api_url = normalize_api_url(
        global
            .url
            .as_deref()
            .or_else(|| saved.as_ref().map(|profile| profile.url.as_str()))
            .unwrap_or(DEFAULT_API_URL),
    );
    let accounts_url = normalize_accounts_url(
        global
            .accounts_url
            .as_deref()
            .or_else(|| {
                saved
                    .as_ref()
                    .and_then(|profile| profile.accounts_url.as_deref())
            })
            .unwrap_or(DEFAULT_ACCOUNTS_URL),
    );
    let value = json!({
        "app_id": APP_ID,
        "name": "Briefcase",
        "client_id": APP_ID,
        "accounts_url": accounts_url,
        "api_url": api_url,
        "web_url": WEB_URL,
        "version": env!("CARGO_PKG_VERSION"),
        "contract_version": briefcase_client::CONTRACT_VERSION,
        "api_version": briefcase_client::API_VERSION,
        "sign_in": {
            "device_flow": true,
            "public_client": true,
            "carbon": "briefcase login",
            "silicon": SILICON_SIGN_IN,
        },
        "install": "silicon-apps install briefcase",
        "docs": "https://docs.briefcase.teamofsilicons.com/",
        "repository": "https://github.com/teamofsilicons/silicon-briefcase",
        "package": "https://crates.io/crates/briefcase-client",
    });
    if output.is_json() {
        output.json(&value);
        return;
    }
    println!("app id      {APP_ID}");
    println!("accounts    {accounts_url}");
    println!("api         {api_url}");
    println!("website     {WEB_URL}");
    println!(
        "version     {} (contract {})",
        env!("CARGO_PKG_VERSION"),
        briefcase_client::CONTRACT_VERSION
    );
    println!("Carbons     briefcase login");
    println!("Silicons    {SILICON_SIGN_IN}");
}

/// `briefcase status`: the profile, the saved sign-in (read from the file) and
/// whether the deployment's contract matches this build.
///
/// # Errors
///
/// Returns a state error when the state cannot be read.
pub async fn status(global: &GlobalArgs, output: Output) -> Result<()> {
    let selection = session::select(global)?;
    let credentials = selection.state.credentials()?;
    let saved = credentials.session(&selection.profile);
    let client = Client::new_unchecked(Config::new(&selection.api_url)?.with_telemetry(false))?;
    let served = client.version().await;
    let agreement = served
        .as_ref()
        .ok()
        .map(briefcase_client::ServiceVersion::check_compatibility);
    if output.is_json() {
        output.json(&json!({
            "profile": selection.profile,
            "state": selection.state.path().display().to_string(),
            "api_url": selection.api_url,
            "accounts_url": selection.accounts_url,
            "signed_in": saved.map(|session| signed_in_json(session, &selection.profile, false)),
            "service": served.as_ref().ok().map(|version| json!({
                "service": version.service,
                "build": version.build,
                "contract_version": version.contract_version,
                "supported_api_versions": version.supported_api_versions,
            })),
            "service_error": served.as_ref().err().map(ToString::to_string),
            "compatible": agreement.as_ref().map(std::result::Result::is_ok),
        }));
        return Ok(());
    }
    println!("profile     {}", selection.profile);
    println!("state       {}", selection.state.path().display());
    println!("briefcase   {}", selection.api_url);
    println!("accounts    {}", selection.accounts_url);
    match saved {
        Some(session) => println!(
            "signed in   as {} ({}), access token until {}",
            session.account.display_id(),
            session.account.uuid,
            timestamp(session.expires_at)
        ),
        None => println!("signed in   no — `briefcase login`"),
    }
    match (&served, agreement) {
        (Ok(version), Some(Ok(()))) => println!(
            "service     {} build {}, contract {} (matches this build)",
            version.service, version.build, version.contract_version
        ),
        (Ok(version), Some(Err(error))) => println!(
            "service     {} build {}, contract {} — MISMATCH: {error}",
            version.service, version.build, version.contract_version
        ),
        (Ok(_), None) => println!("service     unknown"),
        (Err(error), _) => println!("service     not reachable: {error}"),
    }
    Ok(())
}
