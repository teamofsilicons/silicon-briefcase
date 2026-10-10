//! Silicon Accounts configuration: where Silicon Accounts lives, who Briefcase
//! is there, how webhook deliveries are verified, and which apps may act for
//! an account here.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    num::NonZeroUsize,
    time::Duration,
};

use secrecy::{ExposeSecret as _, SecretString};
use url::Url;

use super::{
    RuntimeEnvironment, SettingsError, duration_millis, duration_secs, invalid, optional, parse_or,
    required_secret,
};
use crate::domain::{actor::is_canonical_iam_application_id, delegation::DELEGATED_SCOPES};

/// Production Silicon Accounts origin.
pub const DEFAULT_ACCOUNTS_URL: &str = "https://accounts.teamofsilicons.com";
/// Briefcase's app id at Silicon Accounts and Silicon Apps.
pub const DEFAULT_APP_ID: &str = "briefcase";
/// Allowed clock difference for a webhook delivery's timestamp.
pub const WEBHOOK_TOLERANCE: Duration = Duration::from_secs(300);

const APP_SECRET_PREFIX: &str = "sa_app_";
const WEBHOOK_SECRET_PREFIX: &str = "whsec_";

/// Silicon Accounts endpoints, Briefcase's app credentials and delegation policy.
#[derive(Clone, Debug)]
pub struct AccountsSettings {
    /// Public Silicon Accounts origin (`ACCOUNTS_URL`). Access tokens carry it
    /// as their issuer.
    pub public_url: Url,
    /// Exact `iss` value accepted on access tokens: the public origin without
    /// a trailing slash.
    pub issuer: String,
    /// Origin Briefcase calls server to server (`ACCOUNTS_API_URL`, defaulting
    /// to the public origin).
    pub api_url: Url,
    /// Briefcase's app id: the `aud` of every access token it accepts.
    pub app_id: String,
    /// Briefcase's app secret, used only for server-to-server calls.
    pub app_secret: SecretString,
    /// Deadline for one Silicon Accounts request.
    pub request_timeout: Duration,
    /// How long a looked-up account (id, kind, custodian) is reused before
    /// Briefcase asks Silicon Accounts again.
    pub account_cache_ttl: Duration,
    /// Which issuing apps may use which delegated scope.
    pub proof_issuers: ProofIssuers,
}

/// Webhook verification for deliveries from Silicon Accounts.
#[derive(Clone, Debug)]
pub struct WebhookSettings {
    /// Current signing secret (`whsec_…`).
    pub signing_secret: SecretString,
    /// The secret before a rotation, accepted until operators remove it.
    pub previous_signing_secret: Option<SecretString>,
    /// Allowed difference between the delivery timestamp and this clock.
    pub tolerance: Duration,
    /// Maximum webhook request body size.
    pub max_body_bytes: NonZeroUsize,
}

impl WebhookSettings {
    /// Every secret a delivery may be signed with, current first.
    pub fn secrets(&self) -> impl Iterator<Item = &SecretString> {
        std::iter::once(&self.signing_secret).chain(self.previous_signing_secret.as_ref())
    }
}

/// Per-scope allow-list of apps that may act for an account at Briefcase.
///
/// Parsed from `BRIEFCASE_PROOF_ISSUERS`: entries separated by commas,
/// semicolons or whitespace, each `scope=app` or `scope=app1|app2`, where the
/// scope is one of Briefcase's delegated scopes. Unset means no app may act
/// for anyone (deny by default).
///
/// `BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS` names the apps (such as `interface`)
/// that, for the scopes they are allowed, reach everything the account can
/// instead of only their own `apps/{app}/` folder in its drive.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProofIssuers {
    by_scope: BTreeMap<String, BTreeSet<String>>,
    first_party: BTreeSet<String>,
}

impl ProofIssuers {
    /// Parses the allow-list.
    ///
    /// # Errors
    ///
    /// Returns a precise reason for an entry without `=`, an unknown scope, or
    /// an issuer that is not a bare app id.
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut by_scope: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in value
            .split(|character: char| matches!(character, ',' | ';') || character.is_whitespace())
            .filter(|entry| !entry.is_empty())
        {
            let Some((scope, issuers)) = entry.split_once('=') else {
                return Err(format!(
                    "entry `{entry}` must be `scope=app` or `scope=app1|app2`, such as `briefcase.uploads.reserve=waveform`"
                ));
            };
            if !DELEGATED_SCOPES.contains(&scope) {
                return Err(format!(
                    "`{scope}` is not a Briefcase delegated scope; use one of {}",
                    DELEGATED_SCOPES.join(", ")
                ));
            }
            let allowed = by_scope.entry(scope.to_owned()).or_default();
            for issuer in issuers.split('|') {
                if !is_canonical_iam_application_id(issuer) {
                    return Err(format!(
                        "`{issuer}` (for scope {scope}) is not a bare app id such as `waveform`"
                    ));
                }
                allowed.insert(issuer.to_owned());
            }
        }
        Ok(Self {
            by_scope,
            first_party: BTreeSet::new(),
        })
    }

    /// Adds the first-party apps, parsed from a list separated by commas,
    /// semicolons or whitespace.
    ///
    /// # Errors
    ///
    /// Returns a precise reason for an entry that is not a bare app id.
    pub fn with_first_party(mut self, value: &str) -> Result<Self, String> {
        for issuer in value
            .split(|character: char| matches!(character, ',' | ';') || character.is_whitespace())
            .filter(|entry| !entry.is_empty())
        {
            if !is_canonical_iam_application_id(issuer) {
                return Err(format!(
                    "`{issuer}` is not a bare app id such as `interface`"
                ));
            }
            self.first_party.insert(issuer.to_owned());
        }
        Ok(self)
    }

    /// Returns whether `issuer` is a first-party app: allowed scopes then
    /// reach everything the account can, not only the app's own folder.
    #[must_use]
    pub fn is_first_party(&self, issuer: &str) -> bool {
        self.first_party.contains(issuer)
    }

    /// The first-party apps.
    pub fn first_party(&self) -> impl Iterator<Item = &str> {
        self.first_party.iter().map(String::as_str)
    }

    /// Returns whether `issuer` may use `scope`.
    #[must_use]
    pub fn allows(&self, scope: &str, issuer: &str) -> bool {
        self.by_scope
            .get(scope)
            .is_some_and(|issuers| issuers.contains(issuer))
    }

    /// Returns whether no app may act for anyone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_scope.is_empty()
    }

    /// The apps allowed for one scope.
    pub fn issuers_for(&self, scope: &str) -> impl Iterator<Item = &str> {
        self.by_scope
            .get(scope)
            .into_iter()
            .flat_map(|issuers| issuers.iter().map(String::as_str))
    }
}

pub(super) fn accounts_settings() -> Result<AccountsSettings, SettingsError> {
    let raw_public = optional("ACCOUNTS_URL").unwrap_or_else(|| DEFAULT_ACCOUNTS_URL.to_owned());
    let public_url = accounts_origin("ACCOUNTS_URL", &raw_public)?;
    let issuer = raw_public.trim().trim_end_matches('/').to_owned();
    let api_url = match optional("ACCOUNTS_API_URL") {
        Some(raw) => accounts_origin("ACCOUNTS_API_URL", &raw)?,
        None => public_url.clone(),
    };
    let app_id = optional("BRIEFCASE_APP_ID").unwrap_or_else(|| DEFAULT_APP_ID.to_owned());
    if !is_canonical_iam_application_id(&app_id) {
        return Err(invalid(
            "BRIEFCASE_APP_ID",
            "must be Briefcase's bare app id at Silicon Accounts, such as `briefcase`",
        ));
    }
    let app_secret = required_secret("BRIEFCASE_APP_SECRET")?;
    validate_prefixed_secret(
        "BRIEFCASE_APP_SECRET",
        &app_secret,
        APP_SECRET_PREFIX,
        "app",
    )?;
    let request_timeout = duration_millis("BRIEFCASE_ACCOUNTS_REQUEST_TIMEOUT_MS", 5_000)?;
    if request_timeout.is_zero() {
        return Err(invalid(
            "BRIEFCASE_ACCOUNTS_REQUEST_TIMEOUT_MS",
            "must be greater than zero",
        ));
    }
    let account_cache_ttl = duration_secs("BRIEFCASE_ACCOUNT_CACHE_TTL_SECONDS", 300)?;
    if account_cache_ttl.is_zero() || account_cache_ttl > Duration::from_hours(24) {
        return Err(invalid(
            "BRIEFCASE_ACCOUNT_CACHE_TTL_SECONDS",
            "must be between 1 and 86400 seconds",
        ));
    }
    let proof_issuers = match optional("BRIEFCASE_PROOF_ISSUERS") {
        Some(value) => ProofIssuers::parse(&value)
            .map_err(|reason| invalid("BRIEFCASE_PROOF_ISSUERS", reason))?,
        None => ProofIssuers::default(),
    };
    let proof_issuers = match optional("BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS") {
        Some(value) => proof_issuers
            .with_first_party(&value)
            .map_err(|reason| invalid("BRIEFCASE_PROOF_FIRST_PARTY_ISSUERS", reason))?,
        None => proof_issuers,
    };
    Ok(AccountsSettings {
        public_url,
        issuer,
        api_url,
        app_id,
        app_secret,
        request_timeout,
        account_cache_ttl,
        proof_issuers,
    })
}

pub(super) fn webhook_settings() -> Result<WebhookSettings, SettingsError> {
    let signing_secret = required_secret("BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET")?;
    validate_prefixed_secret(
        "BRIEFCASE_ACCOUNTS_WEBHOOK_SECRET",
        &signing_secret,
        WEBHOOK_SECRET_PREFIX,
        "webhook signing",
    )?;
    let previous_signing_secret = optional("BRIEFCASE_ACCOUNTS_WEBHOOK_PREVIOUS_SECRET")
        .map(SecretString::from)
        .map(|secret| {
            validate_prefixed_secret(
                "BRIEFCASE_ACCOUNTS_WEBHOOK_PREVIOUS_SECRET",
                &secret,
                WEBHOOK_SECRET_PREFIX,
                "webhook signing",
            )
            .map(|()| secret)
        })
        .transpose()?;
    Ok(WebhookSettings {
        signing_secret,
        previous_signing_secret,
        tolerance: WEBHOOK_TOLERANCE,
        max_body_bytes: parse_or("BRIEFCASE_WEBHOOK_MAX_BODY_BYTES", "262144")?,
    })
}

/// Production refuses plain http even for loopback.
pub(super) fn validate_accounts_environment(
    environment: RuntimeEnvironment,
    accounts: &AccountsSettings,
) -> Result<(), SettingsError> {
    if environment != RuntimeEnvironment::Production {
        return Ok(());
    }
    for (name, url) in [
        ("ACCOUNTS_URL", &accounts.public_url),
        ("ACCOUNTS_API_URL", &accounts.api_url),
    ] {
        if url.scheme() != "https" {
            return Err(invalid(name, "must use https in production"));
        }
    }
    Ok(())
}

/// Returns whether plain http to this host keeps traffic on this machine.
#[must_use]
pub fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

fn accounts_origin(name: &'static str, raw: &str) -> Result<Url, SettingsError> {
    let url = Url::parse(raw.trim()).map_err(|_| {
        invalid(
            name,
            "must be an absolute URL such as https://accounts.teamofsilicons.com",
        )
    })?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(name, "must not contain embedded credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(name, "must not contain a query or fragment"));
    }
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(&url) => {}
        "http" => {
            return Err(invalid(
                name,
                "uses plain http for a host that is not this machine; use https, or http only for localhost, 127.0.0.1 or ::1",
            ));
        }
        _ => return Err(invalid(name, "must use https (or http for this machine)")),
    }
    if url.host_str().is_none() {
        return Err(invalid(name, "must include a host"));
    }
    Ok(url)
}

fn validate_prefixed_secret(
    name: &'static str,
    secret: &SecretString,
    prefix: &str,
    what: &str,
) -> Result<(), SettingsError> {
    let value = secret.expose_secret();
    let body = value.strip_prefix(prefix).unwrap_or_default();
    if !value.starts_with(prefix)
        || body.len() < 16
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid(
            name,
            format!(
                "must be Briefcase's {what} secret from Silicon Accounts: `{prefix}` followed by at least 16 letters, digits, `_` or `-`"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use url::Url;

    use super::{ProofIssuers, accounts_origin, is_loopback_host, validate_prefixed_secret};

    #[test]
    fn proof_issuers_are_scope_scoped_and_deny_by_default() -> Result<(), String> {
        let empty = ProofIssuers::default();
        assert!(empty.is_empty());
        assert!(!empty.allows("briefcase.files.read", "waveform"));

        let issuers = ProofIssuers::parse(
            "briefcase.uploads.reserve=waveform|extend, briefcase.files.read=waveform;\n briefcase.entries.list=interface",
        )?;
        assert!(issuers.allows("briefcase.uploads.reserve", "waveform"));
        assert!(issuers.allows("briefcase.uploads.reserve", "extend"));
        assert!(issuers.allows("briefcase.files.read", "waveform"));
        assert!(!issuers.allows("briefcase.files.read", "extend"));
        assert!(!issuers.allows("briefcase.uploads.commit", "waveform"));
        assert_eq!(
            issuers
                .issuers_for("briefcase.uploads.reserve")
                .collect::<Vec<_>>(),
            ["extend", "waveform"]
        );
        Ok(())
    }

    #[test]
    fn first_party_issuers_are_bare_app_ids() -> Result<(), String> {
        let issuers = ProofIssuers::parse("briefcase.entries.list=interface|waveform")?
            .with_first_party("interface")?;
        assert!(issuers.is_first_party("interface"));
        assert!(!issuers.is_first_party("waveform"));
        assert_eq!(issuers.first_party().collect::<Vec<_>>(), vec!["interface"]);
        assert!(!ProofIssuers::default().is_first_party("interface"));
        assert_eq!(
            ProofIssuers::default().with_first_party("Interface").err(),
            Some("`Interface` is not a bare app id such as `interface`".to_owned())
        );
        Ok(())
    }

    #[test]
    fn proof_issuers_reject_unknown_scopes_and_malformed_entries() {
        let unknown = ProofIssuers::parse("briefcase.files.delete=waveform").err();
        assert!(unknown.is_some_and(|reason| reason.starts_with(
            "`briefcase.files.delete` is not a Briefcase delegated scope; use one of"
        )));
        assert_eq!(
            ProofIssuers::parse("waveform").err().as_deref(),
            Some(
                "entry `waveform` must be `scope=app` or `scope=app1|app2`, such as `briefcase.uploads.reserve=waveform`"
            )
        );
        assert_eq!(
            ProofIssuers::parse("briefcase.files.read=Waveform")
                .err()
                .as_deref(),
            Some(
                "`Waveform` (for scope briefcase.files.read) is not a bare app id such as `waveform`"
            )
        );
    }

    #[test]
    fn plain_http_is_only_for_this_machine() -> anyhow::Result<()> {
        for loopback in [
            "http://localhost:9590",
            "http://127.0.0.1:9589",
            "http://[::1]:9589",
            "http://accounts.localhost",
        ] {
            assert!(is_loopback_host(&Url::parse(loopback)?), "{loopback}");
            assert!(
                accounts_origin("ACCOUNTS_URL", loopback).is_ok(),
                "{loopback}"
            );
        }
        let remote = accounts_origin("ACCOUNTS_URL", "http://accounts.teamofsilicons.com");
        assert_eq!(
            remote.err().map(|error| error.to_string()).as_deref(),
            Some(
                "invalid environment variable ACCOUNTS_URL: uses plain http for a host that is not this machine; use https, or http only for localhost, 127.0.0.1 or ::1"
            )
        );
        assert!(accounts_origin("ACCOUNTS_URL", "https://accounts.teamofsilicons.com").is_ok());
        assert!(accounts_origin("ACCOUNTS_URL", "https://user:pw@accounts.example").is_err());
        assert!(accounts_origin("ACCOUNTS_URL", "https://accounts.example/?x=1").is_err());
        assert!(accounts_origin("ACCOUNTS_URL", "ftp://accounts.example").is_err());
        Ok(())
    }

    #[test]
    fn secrets_must_have_their_silicon_accounts_prefix() {
        let app = SecretString::from(format!("sa_app_{}", "a".repeat(40)));
        assert!(validate_prefixed_secret("S", &app, "sa_app_", "app").is_ok());
        let wrong = SecretString::from(format!("ask_{}", "a".repeat(43)));
        assert_eq!(
            validate_prefixed_secret("BRIEFCASE_APP_SECRET", &wrong, "sa_app_", "app")
                .err()
                .map(|error| error.to_string())
                .as_deref(),
            Some(
                "invalid environment variable BRIEFCASE_APP_SECRET: must be Briefcase's app secret from Silicon Accounts: `sa_app_` followed by at least 16 letters, digits, `_` or `-`"
            )
        );
        let short = SecretString::from("whsec_short".to_owned());
        assert!(validate_prefixed_secret("S", &short, "whsec_", "webhook signing").is_err());
    }
}
