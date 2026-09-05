//! Everything a client needs to be told, and nothing it should remember.
//!
//! The package holds no login session or API cache. Whatever a caller wants
//! remembered between runs — tokens, environment UUID-to-key mappings, a
//! default organization — belongs to the caller. Dependency maintenance is the
//! deliberate exception: its default-on best-effort updater can advance the
//! consuming Cargo lockfile and has explicit config/environment opt-outs.

use std::{path::PathBuf, time::Duration};

use secrecy::SecretString;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
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

/// A 32-character root key selecting one isolated Briefcase testing environment.
///
/// This selects a data plane; it does not replace the IAM bearer credential used
/// inside that plane. Its `Debug` representation is always redacted.
#[derive(Clone)]
pub struct EnvironmentKey(SecretString);

impl EnvironmentKey {
    /// Validates the fixed 32-character alphanumeric wire form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] when the value is not exactly 32 ASCII
    /// alphanumeric characters.
    pub fn new(key: impl Into<String>) -> Result<Self, Error> {
        let key = key.into();
        if key.len() != 32 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(Error::Configuration(
                "a testing environment key is exactly 32 alphanumeric characters".into(),
            ));
        }
        Ok(Self(SecretString::from(key)))
    }

    pub(crate) fn expose(&self) -> &str {
        secrecy::ExposeSecret::expose_secret(&self.0)
    }

    /// Exposes the root key when a caller must persist or deliberately print it.
    ///
    /// Treat the returned value as a credential. Normal request construction
    /// does not need this method; [`Config::with_environment`] sends it safely.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.expose()
    }
}

impl std::str::FromStr for EnvironmentKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl std::fmt::Debug for EnvironmentKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnvironmentKey(<redacted>)")
    }
}

impl Serialize for EnvironmentKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for EnvironmentKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let key = String::deserialize(deserializer)?;
        Self::new(key).map_err(D::Error::custom)
    }
}

/// IAM's 32-character root key for the testing plane paired with Briefcase.
///
/// This is intentionally distinct from [`EnvironmentKey`]: confusing the IAM
/// selector with Briefcase's own root key would cross a security boundary.
#[derive(Clone)]
pub struct IamEnvironmentKey(SecretString);

impl IamEnvironmentKey {
    /// Validates IAM's exact environment-key wire form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] unless the key is exactly 32 ASCII
    /// alphanumeric characters.
    pub fn new(key: impl Into<String>) -> Result<Self, Error> {
        let key = key.into();
        if key.len() != 32 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(Error::Configuration(
                "an IAM testing environment key is exactly 32 alphanumeric characters".into(),
            ));
        }
        Ok(Self(SecretString::from(key)))
    }

    fn expose(&self) -> &str {
        secrecy::ExposeSecret::expose_secret(&self.0)
    }
}

impl std::str::FromStr for IamEnvironmentKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl std::fmt::Debug for IamEnvironmentKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("IamEnvironmentKey(<redacted>)")
    }
}

impl Serialize for IamEnvironmentKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for IamEnvironmentKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let key = String::deserialize(deserializer)?;
        Self::new(key).map_err(D::Error::custom)
    }
}

/// Canonical organization-qualified IAM Application ID.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApplicationId(String);

impl ApplicationId {
    /// Validates `{org_id}>{handle}` without normalizing caller input.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] when either component violates IAM's
    /// canonical public identifier grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        let valid = value.matches('>').count() == 1
            && value.split_once('>').is_some_and(|(organization, handle)| {
                valid_handle(organization, 50, false) && valid_handle(handle, 80, true)
            });
        if !valid {
            return Err(Error::Configuration(
                "IAM Application ID must be canonical {org_id}>{handle}".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Returns the canonical identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for ApplicationId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl std::fmt::Display for ApplicationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ApplicationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ApplicationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// Test-only IAM Application secret used by the Briefcase backend.
#[derive(Clone)]
pub struct IamApplicationSecret(SecretString);

impl IamApplicationSecret {
    /// Validates IAM's fixed `ask_` credential wire form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for an invalid prefix, length, or
    /// URL-safe secret alphabet.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.len() != 47
            || !value.starts_with("ask_")
            || !value[4..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(Error::Configuration(
                "an IAM Application secret must use the fixed ask_ wire form".into(),
            ));
        }
        Ok(Self(SecretString::from(value)))
    }

    fn expose(&self) -> &str {
        secrecy::ExposeSecret::expose_secret(&self.0)
    }
}

impl std::str::FromStr for IamApplicationSecret {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl std::fmt::Debug for IamApplicationSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("IamApplicationSecret(<redacted>)")
    }
}

impl Serialize for IamApplicationSecret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for IamApplicationSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

fn valid_handle(value: &str, maximum: usize, first_letter: bool) -> bool {
    (3..=maximum).contains(&value.len())
        && (!first_letter || value.as_bytes().first().is_some_and(u8::is_ascii_lowercase))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

/// The credential a client presents.
#[derive(Clone)]
pub enum Credential {
    /// An IAM access token for a Carbon or Silicon.
    ///
    /// This is the only credential the contracted API accepts.
    Bearer(SecretString),
    /// No credential.
    ///
    /// Useful for reading `GET /api/version` and for an application client
    /// that only ever calls the on-behalf-of endpoint, which carries its own
    /// single-use proof per request.
    None,
}

impl Credential {
    /// Builds a bearer credential from a token.
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

/// How to reach one Briefcase deployment, as one organization.
#[derive(Clone, Debug)]
pub struct Config {
    pub(crate) api_base: Url,
    pub(crate) origin: Url,
    pub(crate) organization: String,
    pub(crate) credential: Credential,
    pub(crate) environment: Option<EnvironmentKey>,
    pub(crate) request_timeout: Duration,
    pub(crate) transfer_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) user_agent: String,
    pub(crate) auto_update: bool,
    pub(crate) update_manifest: Option<PathBuf>,
}

impl Config {
    /// Starts a configuration for one deployment and organization.
    ///
    /// The base URL is the versioned API base, such as
    /// `https://backend.briefcase.teamofsilicons.com/api/v1/`. A missing
    /// trailing slash is added rather than silently swallowing the last path
    /// segment.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] when the URL cannot be parsed, is not
    /// the exact API base this build speaks, uses insecure HTTP outside the
    /// local machine, or the organization is empty.
    pub fn new(base_url: &str, organization: impl Into<String>) -> Result<Self, Error> {
        let organization = organization.into();
        if organization.trim().is_empty() {
            return Err(Error::Configuration(
                "organization must not be empty".into(),
            ));
        }
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
            organization,
            credential: Credential::None,
            environment: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            transfer_timeout: DEFAULT_TRANSFER_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            user_agent: concat!("briefcase-client/", env!("CARGO_PKG_VERSION")).to_owned(),
            auto_update: true,
            update_manifest: None,
        })
    }

    /// Presents an IAM access token on every contracted request.
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

    /// Runs every subsequent request inside one testing environment.
    ///
    /// The environment key is sent separately from the IAM bearer credential,
    /// because selecting a plane never authenticates a Carbon or Silicon.
    #[must_use]
    pub fn with_environment(mut self, environment: EnvironmentKey) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Returns this configuration to the production data plane.
    #[must_use]
    pub fn without_environment(mut self) -> Self {
        self.environment = None;
        self
    }

    /// Enables or disables the default-on best-effort crates.io updater.
    #[must_use]
    pub const fn with_auto_update(mut self, enabled: bool) -> Self {
        self.auto_update = enabled;
        self
    }

    /// Selects the Cargo manifest whose lockfile automatic updates maintain.
    #[must_use]
    pub fn with_update_manifest(mut self, manifest: impl Into<PathBuf>) -> Self {
        self.update_manifest = Some(manifest.into());
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

    /// Returns the organization every request is scoped to.
    #[must_use]
    pub fn organization(&self) -> &str {
        &self.organization
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

    /// Returns the selected testing environment, when this is not production.
    #[must_use]
    pub const fn environment(&self) -> Option<&EnvironmentKey> {
        self.environment.as_ref()
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApplicationId, Config, Credential, EnvironmentKey, IamApplicationSecret, IamEnvironmentKey,
    };

    #[test]
    fn a_base_url_without_a_trailing_slash_keeps_its_last_segment() {
        let config = Config::new("https://briefcase.example/api/v1", "tos").unwrap();
        assert_eq!(
            config.api_base.as_str(),
            "https://briefcase.example/api/v1/"
        );
        assert_eq!(config.origin.as_str(), "https://briefcase.example/");
    }

    #[test]
    fn only_exact_secure_api_bases_and_real_organizations_are_accepted() {
        assert!(Config::new("ftp://briefcase.example/api/v1/", "tos").is_err());
        assert!(Config::new("not a url", "tos").is_err());
        assert!(Config::new("https://briefcase.example/api/v1/", "  ").is_err());
        assert!(Config::new("http://briefcase.example/api/v1/", "tos").is_err());
        assert!(Config::new("https://briefcase.example/api/v2/", "tos").is_err());
        assert!(Config::new("https://briefcase.example/proxy/api/v1/", "tos").is_err());
        assert!(Config::new("https://briefcase.example/api/v1/?tenant=tos", "tos").is_err());
        assert!(Config::new("https://briefcase.example/api/v1/#fragment", "tos").is_err());
        assert!(Config::new("https://user@briefcase.example/api/v1/", "tos").is_err());

        assert!(Config::new("http://localhost:3000/api/v1/", "tos").is_ok());
        assert!(Config::new("http://127.0.0.1:3000/api/v1/", "tos").is_ok());
        assert!(Config::new("http://[::1]:3000/api/v1/", "tos").is_ok());
    }

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let config = Config::new("https://briefcase.example/api/v1/", "tos")
            .unwrap()
            .with_token("secret-token-value");
        let rendered = format!("{config:?}");

        assert!(!rendered.contains("secret-token-value"));
        assert!(rendered.contains("<redacted>"));
        assert!(matches!(config.credential, Credential::Bearer(_)));
    }

    #[test]
    fn environment_keys_are_fixed_length_and_redacted() {
        let key = EnvironmentKey::new("A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6").unwrap();
        assert_eq!(format!("{key:?}"), "EnvironmentKey(<redacted>)");
        assert!(EnvironmentKey::new("short").is_err());
        assert!(EnvironmentKey::new(format!("{}-", "a".repeat(31))).is_err());
    }

    #[test]
    fn iam_test_credentials_and_qualified_ids_fail_closed() {
        let iam_key = IamEnvironmentKey::new("A".repeat(32)).unwrap();
        assert_eq!(format!("{iam_key:?}"), "IamEnvironmentKey(<redacted>)");
        let secret = IamApplicationSecret::new(format!("ask_{}", "a".repeat(43))).unwrap();
        assert_eq!(format!("{secret:?}"), "IamApplicationSecret(<redacted>)");

        assert_eq!(
            ApplicationId::new("acme>briefcase").unwrap().as_str(),
            "acme>briefcase"
        );
        assert!(ApplicationId::new("briefcase").is_err());
        assert!(ApplicationId::new("Acme>briefcase").is_err());
        assert!(IamApplicationSecret::new("ask_short").is_err());
    }

    #[test]
    fn test_secrets_stay_out_of_debug_output() {
        let input = crate::TestingEnvironmentCreate::new(
            "test",
            uuid::Uuid::from_u128(7),
            IamEnvironmentKey::new("i".repeat(32)).unwrap(),
            ApplicationId::new("tos>briefcase").unwrap(),
            IamApplicationSecret::new(format!("ask_{}", "s".repeat(43))).unwrap(),
        );
        let rendered = format!("{input:?}");
        assert!(!rendered.contains(&"i".repeat(32)));
        assert!(!rendered.contains(&format!("ask_{}", "s".repeat(43))));
        assert!(rendered.contains("<redacted>"));
    }
}
