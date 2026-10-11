//! Everything a client needs to be told, and nothing it should remember.
//!
//! The package holds no sign-in session or API cache. Whatever a caller wants
//! remembered between runs — tokens, which account it is — belongs to the
//! caller. Dependency updates also belong to the consuming project; API calls
//! never change its manifest or lockfile.

use std::time::Duration;

use secrecy::SecretString;
use url::{Host, Url};

use crate::{contract::API_VERSION, error::Error};

/// Default deadline for an ordinary request.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Default deadline for uploading or downloading bytes.
///
/// A whole file travels in one request, so this is deliberately generous.
pub const DEFAULT_TRANSFER_TIMEOUT: Duration = Duration::from_mins(15);
/// Default deadline for establishing a connection.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The hosted Briefcase API.
pub const DEFAULT_API_URL: &str = "https://api.briefcase.teamofsilicons.com/api/v1/";

/// The credential a client presents.
#[derive(Clone)]
pub enum Credential {
    /// A Silicon Accounts access token issued to Briefcase (`aud = briefcase`)
    /// for a Carbon or Silicon.
    Bearer(SecretString),
    /// No credential.
    ///
    /// Enough for `GET /api/version`, `GET /api/v1/accounts`, anyone-with-the-link
    /// reads, and an app that only calls the delegated operations, each of
    /// which carries its own proof.
    None,
}

impl Credential {
    /// Builds a bearer credential from a Silicon Accounts access token.
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer(SecretString::from(token.into()))
    }

    /// Returns whether a credential is present.
    #[must_use]
    pub const fn is_present(&self) -> bool {
        matches!(self, Self::Bearer(_))
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer(_) => formatter.write_str("Credential::Bearer(<redacted>)"),
            Self::None => formatter.write_str("Credential::None"),
        }
    }
}

/// How to reach one Briefcase deployment.
#[derive(Clone, Debug)]
pub struct Config {
    pub(crate) api_base: Url,
    pub(crate) origin: Url,
    pub(crate) credential: Credential,
    pub(crate) request_timeout: Duration,
    pub(crate) transfer_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) user_agent: String,
}

impl Config {
    /// Starts a configuration for one deployment.
    ///
    /// The base URL is the versioned API base, such as
    /// `https://api.briefcase.teamofsilicons.com/api/v1/`
    /// ([`DEFAULT_API_URL`]). A missing trailing slash is added rather than
    /// silently swallowing the last path segment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] when the URL cannot be parsed, is not
    /// the exact API base this build speaks, carries credentials, a query or a
    /// fragment, or uses plain HTTP for a host other than this machine.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let normalized = if base_url.ends_with('/') {
            base_url.to_owned()
        } else {
            format!("{base_url}/")
        };
        let api_base = Url::parse(&normalized)
            .map_err(|error| Error::Configuration(format!("base URL is invalid: {error}")))?;
        if !matches!(api_base.scheme(), "http" | "https") {
            return Err(Error::Configuration(
                "base URL must be http or https".into(),
            ));
        }
        if api_base.scheme() == "http" && !is_loopback(&api_base) {
            return Err(Error::Configuration(
                "base URL must use https; http is allowed only for localhost or a loopback IP"
                    .into(),
            ));
        }
        if !api_base.username().is_empty() || api_base.password().is_some() {
            return Err(Error::Configuration(
                "base URL must not contain user information".into(),
            ));
        }
        if api_base.query().is_some() || api_base.fragment().is_some() {
            return Err(Error::Configuration(
                "base URL must not contain a query or fragment".into(),
            ));
        }
        let expected_path = format!("/api/{API_VERSION}/");
        if api_base.path() != expected_path {
            return Err(Error::Configuration(format!(
                "base URL path must be exactly {expected_path}"
            )));
        }
        let mut origin = api_base.clone();
        origin.set_path("/");
        origin.set_query(None);
        origin.set_fragment(None);

        Ok(Self {
            api_base,
            origin,
            credential: Credential::None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            transfer_timeout: DEFAULT_TRANSFER_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            user_agent: concat!("briefcase-client/", env!("CARGO_PKG_VERSION")).to_owned(),
        })
    }

    /// Presents a Silicon Accounts access token issued to Briefcase on every
    /// contracted request (`Authorization: Bearer`).
    #[must_use]
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.credential = Credential::bearer(token);
        self
    }

    /// Presents an already-built credential.
    #[must_use]
    pub fn with_credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    /// Does nothing: telemetry was removed from Briefcase.
    ///
    /// Every request opts out of telemetry (`x-briefcase-telemetry: off`).
    #[deprecated(
        since = "4.1.0",
        note = "Telemetry was removed from Briefcase; this does nothing."
    )]
    #[must_use]
    pub const fn with_telemetry(self, _enabled: bool) -> Self {
        self
    }

    /// Does nothing: telemetry was removed from Briefcase.
    #[deprecated(
        since = "4.1.0",
        note = "Telemetry was removed from Briefcase; this does nothing."
    )]
    #[must_use]
    #[allow(deprecated)]
    pub const fn with_telemetry_source(self, _source: crate::telemetry::Source) -> Self {
        self
    }

    /// Sets the deadline for ordinary requests.
    #[must_use]
    pub const fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sets the deadline for requests that move file bytes.
    #[must_use]
    pub const fn with_transfer_timeout(mut self, timeout: Duration) -> Self {
        self.transfer_timeout = timeout;
        self
    }

    /// Sets the deadline for establishing a connection.
    #[must_use]
    pub const fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Adds a product identity to the user agent this client sends.
    #[must_use]
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Returns the versioned API base.
    #[must_use]
    pub const fn api_base(&self) -> &Url {
        &self.api_base
    }

    /// Returns the canonical deployment origin, without an API path.
    #[must_use]
    pub const fn origin(&self) -> &Url {
        &self.origin
    }

    /// Returns the credential this configuration presents.
    #[must_use]
    pub const fn credential(&self) -> &Credential {
        &self.credential
    }
}

pub(crate) fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Credential, DEFAULT_API_URL};

    #[test]
    fn a_base_url_without_a_trailing_slash_keeps_its_last_segment() {
        let config = Config::new("https://briefcase.example/api/v1").unwrap();
        assert_eq!(
            config.api_base.as_str(),
            "https://briefcase.example/api/v1/"
        );
        assert_eq!(config.origin.as_str(), "https://briefcase.example/");
        assert!(Config::new(DEFAULT_API_URL).is_ok());
    }

    #[test]
    fn only_exact_secure_api_bases_are_accepted() {
        assert!(Config::new("ftp://briefcase.example/api/v1/").is_err());
        assert!(Config::new("not a url").is_err());
        assert!(Config::new("http://briefcase.example/api/v1/").is_err());
        assert!(Config::new("https://briefcase.example/api/v2/").is_err());
        assert!(Config::new("https://briefcase.example/proxy/api/v1/").is_err());
        assert!(Config::new("https://briefcase.example/api/v1/?drive=x").is_err());
        assert!(Config::new("https://briefcase.example/api/v1/#fragment").is_err());
        assert!(Config::new("https://user@briefcase.example/api/v1/").is_err());

        assert!(Config::new("http://localhost:4101/api/v1/").is_ok());
        assert!(Config::new("http://127.0.0.1:4101/api/v1/").is_ok());
        assert!(Config::new("http://[::1]:4101/api/v1/").is_ok());
    }

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let config = Config::new("https://briefcase.example/api/v1/")
            .unwrap()
            .with_token("secret-token-value");
        let rendered = format!("{config:?}");

        assert!(!rendered.contains("secret-token-value"));
        assert!(rendered.contains("<redacted>"));
        assert!(matches!(config.credential, Credential::Bearer(_)));
        assert!(config.credential().is_present());
    }
}
