//! Signing an account in to Briefcase at Silicon Accounts, as Briefcase's own
//! public client (enable the `accounts` feature).
//!
//! Briefcase's own tools — the `briefcase` command, a desktop app — sign
//! accounts in without a secret:
//!
//! - **Carbons** with the device flow: [`SignIn::start_device_sign_in`] returns
//!   a code and a page; the Carbon approves it on the account site while
//!   [`SignIn::finish_device_sign_in`] polls (honouring `interval` and
//!   `slow_down`, and giving up when the code expires after ten minutes).
//! - **Silicons** (and Carbons holding one) by exchanging a short-lived token
//!   from `silicon-accounts login --app briefcase -q` with
//!   [`SignIn::exchange_short_lived_token`]. The token works once, for two
//!   minutes, and only for Briefcase.
//!
//! Both return [`SignedIn`]: a 30-minute access token for Briefcase
//! (`aud = briefcase`), a rotating refresh token (`sar_…`) and the account.
//! Every call is stateless: nothing is remembered. Storing the tokens is the
//! caller's job, and so is refreshing **single-flight**: every refresh returns
//! a new refresh token, and presenting a used one ends the whole sign-in. A
//! public-client refresh is not idempotent, so persist the new pair before
//! using it and never retry an uncertain refresh blindly.
//!
//! ```no_run
//! # async fn example() -> Result<(), briefcase_client::accounts::SignInError> {
//! use briefcase_client::accounts::{DeviceRequest, SignIn};
//!
//! let sign_in = SignIn::new("https://accounts.teamofsilicons.com")?;
//! let device = sign_in.start_device_sign_in(&DeviceRequest::default()).await?;
//! eprintln!("Open {} and enter {}", device.verification_uri, device.user_code);
//! let signed_in = sign_in.finish_device_sign_in(&device, |_| {}).await?;
//! println!("signed in as {}", signed_in.account.display_id());
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use time::OffsetDateTime;
use url::{Host, Url};

use crate::models::{AccountKind, AccountRef};

/// The Silicon Accounts deployment Briefcase signs accounts in at.
pub const DEFAULT_ACCOUNTS_URL: &str = "https://accounts.teamofsilicons.com";
/// Briefcase's app id at Silicon Accounts: the `client_id` of its public client
/// and the audience of its access tokens.
pub const APP_ID: &str = "briefcase";
/// The OAuth grant type of a short-lived token exchange.
pub const SLT_GRANT_TYPE: &str = "urn:silicon:params:oauth:grant-type:slt";
/// The command a Silicon runs to mint a short-lived token for Briefcase.
pub const MINT_COMMAND: &str = "silicon-accounts login --app briefcase -q";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How much longer to wait after every `slow_down` (RFC 8628 §3.5).
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// Signs accounts in to one app at one Silicon Accounts deployment, as the
/// app's public client (`client_id` alone, never a secret).
pub struct SignIn {
    accounts: silicon_accounts_client::AccountsClient,
    http: reqwest::Client,
    base: Url,
    app_id: String,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignIn")
            .field("accounts_url", &self.base.as_str())
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

impl SignIn {
    /// Signs accounts in to Briefcase at `accounts_url`.
    ///
    /// # Errors
    ///
    /// Returns [`SignInError::InvalidInput`] for a URL that is not http(s),
    /// carries credentials, a query or a fragment, or uses plain http for a host
    /// other than this machine.
    pub fn new(accounts_url: &str) -> Result<Self, SignInError> {
        Self::for_app(accounts_url, APP_ID)
    }

    /// Signs accounts in to another app that turned on `device_flow` or
    /// `public_client` (tests and forks; Briefcase's tools use [`SignIn::new`]).
    ///
    /// # Errors
    ///
    /// Returns [`SignInError::InvalidInput`] for an unusable URL or app id.
    pub fn for_app(accounts_url: &str, app_id: &str) -> Result<Self, SignInError> {
        let base = accounts_base(accounts_url)?;
        let app_id = app_id.trim();
        if app_id.is_empty()
            || !app_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(SignInError::invalid(
                format!(
                    "`{app_id}` is not an app id: app ids are bare lowercase names such as `briefcase`."
                ),
                "Pass the app id exactly as Silicon Apps shows it.",
            ));
        }
        let loopback = is_loopback(&base);
        let accounts = silicon_accounts_client::AccountsClient::builder()
            .base_url(base.as_str().trim_end_matches('/'))
            .allow_insecure_http(loopback)
            .user_agent(concat!("briefcase-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| SignInError::invalid(error.to_string(), "Check ACCOUNTS_URL."))?;
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // A token request must never follow a redirect somewhere else.
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent(concat!("briefcase-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                SignInError::invalid(
                    format!("the HTTP client could not be built: {error}"),
                    "This is a bug in the client; report it with `briefcase report`.",
                )
            })?;
        Ok(Self {
            accounts,
            http,
            base,
            app_id: app_id.to_owned(),
        })
    }

    /// The Silicon Accounts URL this signs in at, without a trailing slash.
    #[must_use]
    pub fn accounts_url(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
    }

    /// The app accounts are signed in to.
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Starts a Carbon's device sign-in (`POST /v1/device/authorize` with the
    /// app's `client_id`). Show [`DeviceSignIn::user_code`] and
    /// [`DeviceSignIn::verification_uri`], then call
    /// [`SignIn::finish_device_sign_in`].
    ///
    /// # Errors
    ///
    /// [`SignInError::NotEnabled`] when the app has not turned on device
    /// sign-ins, [`SignInError::Accounts`] for other refusals (for example
    /// `invalid_scope` when `scope` names a detail the app does not ask for),
    /// [`SignInError::Unreachable`] when Silicon Accounts cannot be reached.
    pub async fn start_device_sign_in(
        &self,
        request: &DeviceRequest,
    ) -> Result<DeviceSignIn, SignInError> {
        let started = tokio::time::Instant::now();
        let authorization = self
            .accounts
            .app_device_authorize(
                &self.app_id,
                request
                    .scope
                    .as_deref()
                    .filter(|scope| !scope.trim().is_empty()),
                request
                    .label
                    .as_deref()
                    .filter(|label| !label.trim().is_empty()),
            )
            .await
            .map_err(|error| SignInError::from_accounts(error, Grant::Device))?;
        Ok(DeviceSignIn {
            user_code: authorization.user_code,
            verification_uri: authorization.verification_uri,
            verification_uri_complete: authorization.verification_uri_complete,
            expires_in: Duration::from_secs(authorization.expires_in.max(1)),
            interval: Duration::from_secs(authorization.interval.max(1)),
            device_code: SecretString::from(authorization.device_code.into_inner()),
            started,
        })
    }

    /// Polls a device sign-in once.
    ///
    /// # Errors
    ///
    /// The same as [`SignIn::finish_device_sign_in`], except that pending,
    /// `slow_down`, denied and expired come back as [`DevicePoll`] values.
    pub async fn poll_device_sign_in(
        &self,
        device: &DeviceSignIn,
    ) -> Result<DevicePoll, SignInError> {
        let answer = self
            .accounts
            .app_device_poll(&self.app_id, device.device_code.expose_secret())
            .await
            .map_err(|error| SignInError::from_accounts(error, Grant::Device))?;
        Ok(match answer {
            silicon_accounts_client::DevicePoll::SlowDown => DevicePoll::SlowDown,
            silicon_accounts_client::DevicePoll::Denied => DevicePoll::Denied,
            silicon_accounts_client::DevicePoll::Expired => DevicePoll::Expired,
            silicon_accounts_client::DevicePoll::Tokens(tokens) => {
                DevicePoll::SignedIn(Box::new(SignedIn::from_response(&tokens)?))
            }
            // `Pending`, and any answer a newer Silicon Accounts adds: keep
            // polling until the code expires.
            _ => DevicePoll::Pending,
        })
    }

    /// Polls a device sign-in until the Carbon approves it, honouring the
    /// polling `interval`, adding five seconds after every `slow_down`, and
    /// giving up when the code expires (ten minutes). A poll that cannot reach
    /// Silicon Accounts, or that it answers with a 5xx or 429, is retried until
    /// then. `on_event` hears about everything that is not the final answer.
    ///
    /// # Errors
    ///
    /// [`SignInError::DeviceDenied`] when the Carbon denied it,
    /// [`SignInError::DeviceExpired`] when the code expired first,
    /// [`SignInError::NotEnabled`] when the app turned device sign-ins off
    /// meanwhile, and [`SignInError::Accounts`] for any other refusal.
    pub async fn finish_device_sign_in(
        &self,
        device: &DeviceSignIn,
        mut on_event: impl FnMut(DeviceEvent),
    ) -> Result<SignedIn, SignInError> {
        let deadline = device.started + device.expires_in;
        let mut interval = device.interval;
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(SignInError::DeviceExpired {
                    user_code: device.user_code.clone(),
                });
            }
            tokio::time::sleep(interval.min(deadline - now)).await;
            if tokio::time::Instant::now() >= deadline {
                return Err(SignInError::DeviceExpired {
                    user_code: device.user_code.clone(),
                });
            }
            match self.poll_device_sign_in(device).await {
                Ok(DevicePoll::SignedIn(signed_in)) => return Ok(*signed_in),
                Ok(DevicePoll::Pending) => on_event(DeviceEvent::Pending),
                Ok(DevicePoll::SlowDown) => {
                    interval += SLOW_DOWN_STEP;
                    on_event(DeviceEvent::SlowDown { interval });
                }
                Ok(DevicePoll::Denied) => {
                    return Err(SignInError::DeviceDenied {
                        user_code: device.user_code.clone(),
                    });
                }
                Ok(DevicePoll::Expired) => {
                    return Err(SignInError::DeviceExpired {
                        user_code: device.user_code.clone(),
                    });
                }
                Err(error) if error.is_transient() => on_event(DeviceEvent::Retrying {
                    reason: error.message(),
                    retry_in: interval,
                }),
                Err(error) => return Err(error),
            }
        }
    }

    /// Exchanges a short-lived token (`slt_…`) for Briefcase tokens with the
    /// app's `client_id` alone (the app turned on `public_client`).
    ///
    /// The token is single use: Silicon Accounts uses it up even when it
    /// refuses the exchange, so a refused or uncertain exchange always needs a
    /// fresh one ([`MINT_COMMAND`]). The token is never logged or kept.
    ///
    /// # Errors
    ///
    /// [`SignInError::ShortLivedTokenRefused`] with the exact
    /// [`SltRefusal`] reason, [`SignInError::NotEnabled`] when the app has not
    /// turned on `public_client`, [`SignInError::InvalidInput`] when the value
    /// is not a short-lived token at all (nothing is sent),
    /// [`SignInError::Unreachable`] when the request got no answer.
    pub async fn exchange_short_lived_token(&self, slt: &str) -> Result<SignedIn, SignInError> {
        let slt = slt.trim();
        if slt.is_empty() {
            return Err(SignInError::invalid(
                "The short-lived token is empty.",
                format!(
                    "Mint one with `{MINT_COMMAND}` and pass it to `briefcase login --slt-stdin`."
                ),
            ));
        }
        if !slt.starts_with("slt_") {
            let what = describe_token(slt);
            return Err(SignInError::ShortLivedTokenRefused {
                reason: SltRefusal::NotAShortLivedToken,
                description: format!(
                    "Briefcase signs in with a Silicon Accounts short-lived token (it starts with slt_), but this is {what}; nothing was sent."
                ),
            });
        }
        let response = self
            .post_form(
                "token",
                &[
                    ("grant_type", SLT_GRANT_TYPE),
                    ("slt", slt),
                    ("client_id", &self.app_id),
                ],
            )
            .await?;
        let tokens = token_answer(response, Grant::ShortLivedToken).await?;
        SignedIn::from_response(&tokens)
    }

    /// Rotates a refresh token with the app's `client_id` alone. The token
    /// passed in stops working: store the returned pair before using it.
    ///
    /// # Errors
    ///
    /// [`SignInError::SignInEnded`] when Silicon Accounts refuses the refresh
    /// token (already used, revoked, expired, or the account signed out or
    /// removed Briefcase): the account must sign in again.
    /// [`SignInError::Unreachable`] when the request got no answer; the token
    /// may or may not have been used, so do not retry it in the same command.
    pub async fn refresh(&self, refresh_token: &str) -> Result<SignedIn, SignInError> {
        let tokens = self
            .accounts
            .refresh_app_public_client(&self.app_id, refresh_token)
            .await
            .map_err(|error| SignInError::from_accounts(error, Grant::Refresh))?;
        SignedIn::from_response(&tokens)
    }

    /// Ends the sign-in behind a refresh token (`POST /v1/oauth/revoke`,
    /// `client_id` alone). Silicon Accounts answers 200 for unknown or already
    /// revoked tokens too; [`Revocation::revoked`] says whether this call ended
    /// a live sign-in. Only this sign-in ends: the account's other sign-ins to
    /// Briefcase (other machines, the website) stay.
    ///
    /// # Errors
    ///
    /// [`SignInError::NotEnabled`] when Silicon Accounts no longer accepts the
    /// app's public client, [`SignInError::Unreachable`] when it cannot be
    /// reached.
    pub async fn revoke(&self, refresh_token: &str) -> Result<Revocation, SignInError> {
        let response = self
            .post_form(
                "revoke",
                &[
                    ("token", refresh_token.trim()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", &self.app_id),
                ],
            )
            .await?;
        let status = response.status();
        let body = response.bytes().await.map_err(unreachable)?;
        if status.is_success() {
            return Ok(serde_json::from_slice(&body).unwrap_or(Revocation {
                revoked: true,
                message: None,
            }));
        }
        Err(oauth_failure(status.as_u16(), &body, Grant::Revoke))
    }

    async fn post_form(
        &self,
        endpoint: &str,
        fields: &[(&str, &str)],
    ) -> Result<reqwest::Response, SignInError> {
        let url = self
            .base
            .join(&format!("v1/oauth/{endpoint}"))
            .map_err(|error| SignInError::invalid(error.to_string(), "Check ACCOUNTS_URL."))?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        self.http
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(unreachable)
    }
}

/// What a device sign-in asks for.
#[derive(Clone, Debug, Default)]
pub struct DeviceRequest {
    /// Details to ask for besides the ones the app requires, space-separated
    /// (for example `"email"`). A detail the app does not ask for is refused
    /// with `invalid_scope`.
    pub scope: Option<String>,
    /// Shown on the approval page and in the account's sessions list, such as
    /// `briefcase CLI on build-7`.
    pub label: Option<String>,
}

/// A started device sign-in. Show [`DeviceSignIn::user_code`] and
/// [`DeviceSignIn::verification_uri`] to the Carbon; the device code itself is
/// never shown or logged.
pub struct DeviceSignIn {
    /// The code the Carbon confirms on the account site, such as `WDJB-MJHT`.
    pub user_code: String,
    /// Where the Carbon approves, such as `https://accounts.teamofsilicons.com/device`.
    pub verification_uri: String,
    /// The same page with the code already filled in.
    pub verification_uri_complete: Option<String>,
    /// How long the code lives (ten minutes).
    pub expires_in: Duration,
    /// The shortest wait between two polls.
    pub interval: Duration,
    device_code: SecretString,
    started: tokio::time::Instant,
}

impl DeviceSignIn {
    /// The page to open in a browser: the one with the code filled in when
    /// Silicon Accounts offered it.
    #[must_use]
    pub fn browser_url(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
    }
}

impl std::fmt::Debug for DeviceSignIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceSignIn")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// One poll of a device sign-in.
#[derive(Debug)]
pub enum DevicePoll {
    /// Not approved yet.
    Pending,
    /// Polling too fast: wait five seconds longer from now on.
    SlowDown,
    /// The Carbon denied the sign-in.
    Denied,
    /// The code expired before it was approved.
    Expired,
    /// Approved.
    SignedIn(Box<SignedIn>),
}

/// What [`SignIn::finish_device_sign_in`] reports while it waits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceEvent {
    /// Still waiting for the Carbon.
    Pending,
    /// Silicon Accounts asked to poll more slowly; this is the new interval.
    SlowDown {
        /// The wait between polls from now on.
        interval: Duration,
    },
    /// A poll could not reach Silicon Accounts (or it answered 5xx/429); the
    /// next poll follows after `retry_in`.
    Retrying {
        /// What went wrong.
        reason: String,
        /// Wait before the next poll.
        retry_in: Duration,
    },
}

/// A completed sign-in: Briefcase tokens for one account.
pub struct SignedIn {
    /// The access token for Briefcase (an `EdDSA` JWT, `aud = briefcase`).
    pub access_token: SecretString,
    /// The rotating refresh token (`sar_…`); every refresh returns a new one.
    pub refresh_token: Option<SecretString>,
    /// How long the access token lives from now (30 minutes).
    pub expires_in: Duration,
    /// When the refresh token family ends; the account signs in again then.
    pub refresh_expires_at: Option<OffsetDateTime>,
    /// The details the account shares with Briefcase (`profile`, `email`…).
    pub scope: Vec<String>,
    /// Who signed in.
    pub account: SignedInAccount,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignedIn")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .field("scope", &self.scope)
            .field("account", &self.account)
            .finish()
    }
}

impl SignedIn {
    fn from_response(
        response: &silicon_accounts_client::TokenResponse,
    ) -> Result<Self, SignInError> {
        let account = response
            .account
            .as_ref()
            .ok_or_else(|| SignInError::Accounts {
                status: 200,
                code: "account_missing".to_owned(),
                message:
                    "Silicon Accounts returned tokens without saying which account they are for."
                        .to_owned(),
                hint: Some(
                    "Sign in again; report it with `briefcase report` if it repeats.".to_owned(),
                ),
            })?;
        let kind = match account.kind.as_str() {
            "silicon" => AccountKind::Silicon,
            _ => AccountKind::Carbon,
        };
        let account = SignedInAccount {
            uuid: account.uuid.clone(),
            id: account.id.clone(),
            kind,
            display_name: Some(account.display_name.clone()).filter(|name| !name.is_empty()),
            pfp_url: Some(account.pfp_url.clone()).filter(|url| !url.is_empty()),
            custodian: account.custodian.as_ref().map(|custodian| AccountRef {
                uuid: custodian.uuid.clone(),
                id: Some(custodian.id.clone()).filter(|id| !id.is_empty()),
            }),
        };
        let scope = response
            .scopes()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
        Ok(Self {
            access_token: SecretString::from(response.access_token.expose().to_owned()),
            refresh_token: response
                .refresh_token
                .as_ref()
                .map(|token| SecretString::from(token.expose().to_owned())),
            expires_in: Duration::from_secs(response.expires_in),
            refresh_expires_at: response.refresh_token_expires_at,
            scope,
            account,
        })
    }
}

/// The account a sign-in is for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedInAccount {
    /// The permanent, case-sensitive account identifier; key on this.
    pub uuid: String,
    /// The current public id, such as `c:ada` or `si:scout`; it can change.
    pub id: String,
    /// Carbon or Silicon.
    pub kind: AccountKind,
    /// The display name, when the account has one.
    pub display_name: Option<String>,
    /// The profile photo, when the account has one.
    pub pfp_url: Option<String>,
    /// A Silicon's custodian (a Carbon).
    pub custodian: Option<AccountRef>,
}

impl SignedInAccount {
    /// The public id, or the uuid while the account has none.
    #[must_use]
    pub fn display_id(&self) -> &str {
        if self.id.is_empty() {
            &self.uuid
        } else {
            &self.id
        }
    }
}

/// What a revocation did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Revocation {
    /// Whether this call ended a live sign-in (false for a token that was
    /// already used, revoked or unknown).
    pub revoked: bool,
    /// Silicon Accounts' explanation when nothing was revoked.
    #[serde(default)]
    pub message: Option<String>,
}

/// Why Silicon Accounts refused a short-lived token. It always uses the
/// token up, so every reason needs a fresh one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SltRefusal {
    /// It was exchanged before; each one works once.
    AlreadyUsed,
    /// It lived its two minutes.
    Expired,
    /// It was minted for another app (named when Silicon Accounts said which).
    WrongApp {
        /// The app it was minted for.
        app: Option<String>,
    },
    /// Mistyped, never issued, or issued by another Silicon Accounts.
    Unknown,
    /// Not a short-lived token at all (an access or refresh token, an STK…).
    NotAShortLivedToken,
    /// The Silicon sign-in that minted it has ended since (its STK was
    /// rotated, or the trusted outside sign-in it came from ended).
    SignInEnded,
    /// Refused for a reason this client does not recognise; read the description.
    Other,
}

impl SltRefusal {
    /// Recognises the reason in Silicon Accounts' `error_description`.
    #[must_use]
    pub fn classify(description: &str) -> Self {
        let text = description.to_ascii_lowercase();
        if text.contains("already used") {
            Self::AlreadyUsed
        } else if text.contains("expired at") || text.contains("short-lived token expired") {
            Self::Expired
        } else if text.contains("was issued for the app") {
            let app = description
                .split('\'')
                .nth(1)
                .filter(|app| !app.is_empty())
                .map(ToOwned::to_owned);
            Self::WrongApp { app }
        } else if text.contains("is not known") {
            Self::Unknown
        } else if text.contains("must be a short-lived token") || text.contains("refresh token") {
            Self::NotAShortLivedToken
        } else if text.contains("rotated its stk")
            || text.contains("stk rotation")
            || text.contains("trusted outside token")
            || text.contains("removed that trust")
        {
            Self::SignInEnded
        } else {
            Self::Other
        }
    }

    /// The stable code, shared by every Silicon app's command-line tool.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AlreadyUsed => "slt_already_used",
            Self::Expired => "slt_expired",
            Self::WrongApp { .. } => "slt_wrong_app",
            Self::Unknown => "slt_unknown",
            Self::NotAShortLivedToken => "slt_not_a_short_lived_token",
            Self::SignInEnded => "slt_sign_in_ended",
            Self::Other => "slt_refused",
        }
    }
}

/// Everything that can go wrong signing in. Each error says what failed and
/// why ([`SignInError::message`]) and what to do next ([`SignInError::hint`]).
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    /// Silicon Accounts refused the short-lived token (and used it up).
    #[error("{description}")]
    ShortLivedTokenRefused {
        /// The recognised reason.
        reason: SltRefusal,
        /// Silicon Accounts' own description.
        description: String,
    },
    /// Silicon Accounts refused the refresh token: the sign-in has ended.
    #[error("{description}")]
    SignInEnded {
        /// Silicon Accounts' own description.
        description: String,
    },
    /// The Carbon denied the device sign-in on the account site.
    #[error("the sign-in request {user_code} was denied on the account site")]
    DeviceDenied {
        /// The code that was denied.
        user_code: String,
    },
    /// The device code expired before it was approved.
    #[error("the sign-in code {user_code} expired before it was approved")]
    DeviceExpired {
        /// The code that expired.
        user_code: String,
    },
    /// The app has not turned on public sign-in for this grant (or Silicon
    /// Accounts does not know the app).
    #[error("{description}")]
    NotEnabled {
        /// Which sign-in: `device` or `short-lived token` or `refresh`.
        grant: &'static str,
        /// Silicon Accounts' own description.
        description: String,
    },
    /// Silicon Accounts answered with another error.
    #[error("{message}")]
    Accounts {
        /// HTTP status.
        status: u16,
        /// The stable error code (`invalid_scope`, `rate_limited`…).
        code: String,
        /// What went wrong and why.
        message: String,
        /// What to do next, when Silicon Accounts said.
        hint: Option<String>,
    },
    /// The request got no answer: Silicon Accounts could not be reached.
    #[error("Silicon Accounts could not be reached: {message}")]
    Unreachable {
        /// What failed at the transport layer.
        message: String,
    },
    /// The input was refused before anything was sent.
    #[error("{message}")]
    InvalidInput {
        /// What is wrong.
        message: String,
        /// How to fix it.
        hint: String,
    },
}

impl SignInError {
    fn invalid(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: message.into(),
            hint: hint.into(),
        }
    }

    /// The stable machine-readable code.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::ShortLivedTokenRefused { reason, .. } => reason.code(),
            Self::SignInEnded { .. } => "sign_in_ended",
            Self::DeviceDenied { .. } => "device_sign_in_denied",
            Self::DeviceExpired { .. } => "device_code_expired",
            Self::NotEnabled { .. } => "public_client_disabled",
            Self::Accounts { code, .. } => code,
            Self::Unreachable { .. } => "accounts_unreachable",
            Self::InvalidInput { .. } => "invalid_input",
        }
    }

    /// What failed and why.
    #[must_use]
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// What to do next.
    #[must_use]
    pub fn hint(&self) -> Option<String> {
        let fresh = format!(
            "Mint a fresh one with `{MINT_COMMAND}` and pass it on standard input: `{MINT_COMMAND} | briefcase login --slt-stdin`."
        );
        Some(match self {
            Self::ShortLivedTokenRefused { reason, .. } => match reason {
                SltRefusal::AlreadyUsed => {
                    format!("Short-lived tokens work once and the exchange used this one. {fresh}")
                }
                SltRefusal::Expired => format!(
                    "Short-lived tokens live two minutes; exchange it right after minting it. {fresh}"
                ),
                SltRefusal::WrongApp { app } => format!(
                    "It was minted for {}, and Briefcase only accepts tokens minted for `briefcase`. {fresh}",
                    app.as_deref().map_or_else(|| "another app".to_owned(), |app| format!("`{app}`"))
                ),
                SltRefusal::Unknown => format!(
                    "Check that it was copied whole and minted by the Silicon Accounts this command signs in at (ACCOUNTS_URL). {fresh}"
                ),
                SltRefusal::NotAShortLivedToken => format!(
                    "Pass the short-lived token (slt_…), not an access, refresh or proof token or an STK. {fresh}"
                ),
                SltRefusal::SignInEnded => format!(
                    "The Silicon's own sign-in ended (its STK was rotated or its trusted outside sign-in ended); sign the Silicon in to Silicon Accounts again, then {}",
                    fresh.to_lowercase()
                ),
                SltRefusal::Other => fresh,
            },
            Self::SignInEnded { .. } => {
                "Sign in again: `briefcase login` (Carbons) or `silicon-accounts login --app briefcase -q | briefcase login --slt-stdin` (Silicons).".to_owned()
            }
            Self::DeviceDenied { .. } => {
                "If that was a mistake, run `briefcase login` again and approve the new code.".to_owned()
            }
            Self::DeviceExpired { .. } => {
                "Run `briefcase login` again and approve the new code within ten minutes.".to_owned()
            }
            Self::NotEnabled { grant, .. } => format!(
                "Briefcase's sign-in setup at Silicon Accounts must allow {grant} sign-ins from its own tools (device_flow and public_client); check ACCOUNTS_URL points at the right Silicon Accounts."
            ),
            Self::Accounts { hint, .. } => return hint.clone(),
            Self::Unreachable { .. } => {
                "Check the network and ACCOUNTS_URL, then try again.".to_owned()
            }
            Self::InvalidInput { hint, .. } => hint.clone(),
        })
    }

    /// Whether trying the same request again later can succeed (no answer,
    /// a 5xx or a rate limit).
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        match self {
            Self::Unreachable { .. } => true,
            Self::Accounts { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }

    fn from_accounts(error: silicon_accounts_client::Error, grant: Grant) -> Self {
        match error {
            silicon_accounts_client::Error::OAuth(oauth) => Self::from_oauth(
                oauth.status,
                &oauth.error,
                &oauth.message(),
                Some(oauth.hint().to_owned()),
                grant,
            ),
            silicon_accounts_client::Error::Api(api) => {
                if matches!(
                    api.code.as_str(),
                    "unauthorized_client" | "invalid_client" | "app_disabled"
                ) {
                    return Self::NotEnabled {
                        grant: grant.label(),
                        description: api.message.clone(),
                    };
                }
                Self::Accounts {
                    status: api.status,
                    code: api.code.clone(),
                    message: api.message.clone(),
                    hint: api.hint.clone(),
                }
            }
            silicon_accounts_client::Error::Http { message, .. } => Self::Unreachable { message },
            other => Self::Accounts {
                status: 0,
                code: "accounts_error".to_owned(),
                message: other.to_string(),
                hint: None,
            },
        }
    }

    fn from_oauth(
        status: u16,
        error: &str,
        description: &str,
        hint: Option<String>,
        grant: Grant,
    ) -> Self {
        match (error, grant) {
            ("invalid_grant", Grant::ShortLivedToken) => Self::ShortLivedTokenRefused {
                reason: SltRefusal::classify(description),
                description: description.to_owned(),
            },
            ("invalid_grant", Grant::Refresh) => Self::SignInEnded {
                description: description.to_owned(),
            },
            ("invalid_client" | "unauthorized_client", _) => Self::NotEnabled {
                grant: grant.label(),
                description: description.to_owned(),
            },
            _ => Self::Accounts {
                status,
                code: error.to_owned(),
                message: description.to_owned(),
                hint,
            },
        }
    }
}

/// Which request an OAuth error answered.
#[derive(Clone, Copy, Debug)]
enum Grant {
    Device,
    ShortLivedToken,
    Refresh,
    Revoke,
}

impl Grant {
    const fn label(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::ShortLivedToken => "short-lived token",
            Self::Refresh => "refresh",
            Self::Revoke => "sign-out",
        }
    }
}

#[derive(Deserialize)]
struct OAuthBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct ApiBody {
    error: ApiBodyError,
}

#[derive(Deserialize)]
struct ApiBodyError {
    code: String,
    message: String,
    #[serde(default)]
    hint: Option<String>,
}

async fn token_answer(
    response: reqwest::Response,
    grant: Grant,
) -> Result<silicon_accounts_client::TokenResponse, SignInError> {
    let status = response.status();
    let body = response.bytes().await.map_err(unreachable)?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|error| SignInError::Accounts {
            status: status.as_u16(),
            code: "unreadable_response".to_owned(),
            message: format!(
                "Silicon Accounts answered with tokens this client cannot read: {error}"
            ),
            hint: Some("Check ACCOUNTS_URL points at Silicon Accounts.".to_owned()),
        });
    }
    Err(oauth_failure(status.as_u16(), &body, grant))
}

fn oauth_failure(status: u16, body: &[u8], grant: Grant) -> SignInError {
    if let Ok(oauth) = serde_json::from_slice::<OAuthBody>(body) {
        let description = oauth
            .error_description
            .filter(|description| !description.trim().is_empty())
            .unwrap_or_else(|| {
                silicon_accounts_client::OAuthError::new(status, oauth.error.clone(), None)
                    .message()
            });
        return SignInError::from_oauth(status, &oauth.error, &description, None, grant);
    }
    if let Ok(api) = serde_json::from_slice::<ApiBody>(body) {
        return SignInError::Accounts {
            status,
            code: api.error.code,
            message: api.error.message,
            hint: api.error.hint,
        };
    }
    SignInError::Accounts {
        status,
        code: if status >= 500 {
            "accounts_unavailable".to_owned()
        } else {
            "accounts_refused".to_owned()
        },
        message: format!("Silicon Accounts answered HTTP {status} without an error body."),
        hint: Some("Check ACCOUNTS_URL points at Silicon Accounts, then try again.".to_owned()),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` adapter, which hands the error over by value"
)]
fn unreachable(error: reqwest::Error) -> SignInError {
    let message = if error.is_timeout() {
        "the request timed out".to_owned()
    } else if error.is_connect() {
        "the connection could not be established".to_owned()
    } else {
        error.to_string()
    };
    SignInError::Unreachable { message }
}

/// Says what a value that is not a short-lived token looks like, by its prefix
/// only, so the message never repeats the value.
fn describe_token(value: &str) -> &'static str {
    let value = value.trim();
    if value.starts_with("sar_") {
        "a refresh token (sar_…)"
    } else if value.starts_with("sapr_") {
        "a proof refresh token (sapr_…)"
    } else if value.starts_with("sap_") {
        "a proof (sap_…)"
    } else if value.starts_with("stk") {
        "a Silicon's STK"
    } else if value.starts_with("eyJ") {
        "an access token (a JWT)"
    } else if value.starts_with("oac_") || value.starts_with("oat_") || value.starts_with("ort_") {
        "a token from the retired sign-in service"
    } else {
        "not a Silicon Accounts token"
    }
}

/// Parses and checks a Silicon Accounts URL: http(s), no credentials, query or
/// fragment, and plain http only for this machine.
fn accounts_base(raw: &str) -> Result<Url, SignInError> {
    let trimmed = raw.trim();
    let with_slash = if trimmed.ends_with('/') {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/")
    };
    let url = Url::parse(&with_slash).map_err(|error| {
        SignInError::invalid(
            format!("`{trimmed}` is not a URL: {error}."),
            format!(
                "Set ACCOUNTS_URL to the Silicon Accounts URL, such as {DEFAULT_ACCOUNTS_URL}."
            ),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SignInError::invalid(
            format!("`{trimmed}` is not an http or https URL."),
            format!(
                "Set ACCOUNTS_URL to the Silicon Accounts URL, such as {DEFAULT_ACCOUNTS_URL}."
            ),
        ));
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(SignInError::invalid(
            format!("`{trimmed}` uses plain http for a host that is not this machine."),
            "Use https; plain http is allowed only for localhost, 127.0.0.1 or ::1.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SignInError::invalid(
            "The Silicon Accounts URL must not contain a user name or password.",
            "Remove the credentials from ACCOUNTS_URL.",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(SignInError::invalid(
            "The Silicon Accounts URL must not contain a query or fragment.",
            "Use the bare origin, such as https://accounts.teamofsilicons.com.",
        ));
    }
    Ok(url)
}

/// Whether a URL names this machine.
#[must_use]
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{SignIn, SignInError, SltRefusal, describe_token};

    #[test]
    fn short_lived_token_refusals_are_recognised_from_silicon_accounts_descriptions() {
        for (description, code) in [
            (
                "The short-lived token was already used; each one works once. Get a new one.",
                "slt_already_used",
            ),
            (
                "The short-lived token expired at 2026-10-10T10:00:00.000Z (they last 120 seconds); get a new one with `silicon-accounts login --app briefcase`.",
                "slt_expired",
            ),
            (
                "The short-lived token was issued for the app 'dm', not for 'briefcase'; get one for 'briefcase' with `silicon-accounts login --app briefcase`.",
                "slt_wrong_app",
            ),
            (
                "The short-lived token is not known: it is mistyped or was never issued.",
                "slt_unknown",
            ),
            (
                "slt must be a short-lived token (it starts with slt_), but this is a refresh token.",
                "slt_not_a_short_lived_token",
            ),
            (
                "The short-lived token was issued by a sign-in of si:scout from a trusted outside token, and that trust no longer exists",
                "slt_sign_in_ended",
            ),
            ("Something new.", "slt_refused"),
        ] {
            assert_eq!(
                SltRefusal::classify(description).code(),
                code,
                "{description}"
            );
        }
        assert_eq!(
            SltRefusal::classify(
                "The short-lived token was issued for the app 'dm', not for 'briefcase'"
            ),
            SltRefusal::WrongApp {
                app: Some("dm".to_owned())
            }
        );
    }

    #[test]
    fn every_refusal_says_how_to_get_a_fresh_token() {
        for reason in [
            SltRefusal::AlreadyUsed,
            SltRefusal::Expired,
            SltRefusal::WrongApp { app: None },
            SltRefusal::Unknown,
            SltRefusal::NotAShortLivedToken,
            SltRefusal::SignInEnded,
            SltRefusal::Other,
        ] {
            let error = SignInError::ShortLivedTokenRefused {
                reason,
                description: "refused".to_owned(),
            };
            let hint = error.hint().unwrap();
            assert!(
                hint.contains("silicon-accounts login --app briefcase -q"),
                "{hint}"
            );
        }
    }

    #[test]
    fn plain_http_is_only_for_this_machine() {
        assert!(SignIn::new("http://localhost:9590").is_ok());
        assert!(SignIn::new("http://127.0.0.1:9589/").is_ok());
        assert!(SignIn::new("http://[::1]:9590").is_ok());
        assert!(SignIn::new("https://accounts.teamofsilicons.com").is_ok());
        assert!(SignIn::new("http://accounts.teamofsilicons.com").is_err());
        assert!(SignIn::new("ftp://accounts.example").is_err());
        assert!(SignIn::new("https://user:pw@accounts.example").is_err());
        assert!(SignIn::new("https://accounts.example/?x=1").is_err());
        assert!(SignIn::for_app("https://accounts.example", "Bad App").is_err());
        assert_eq!(
            SignIn::new("https://accounts.teamofsilicons.com/")
                .unwrap()
                .accounts_url(),
            "https://accounts.teamofsilicons.com"
        );
    }

    #[test]
    fn a_misplaced_token_is_described_by_its_prefix_only() {
        assert_eq!(describe_token("sar_secret"), "a refresh token (sar_…)");
        assert_eq!(describe_token("sap_secret"), "a proof (sap_…)");
        assert_eq!(
            describe_token("sapr_secret"),
            "a proof refresh token (sapr_…)"
        );
        assert_eq!(describe_token("eyJhbGciOi"), "an access token (a JWT)");
        assert_eq!(describe_token("hello"), "not a Silicon Accounts token");
    }
}
