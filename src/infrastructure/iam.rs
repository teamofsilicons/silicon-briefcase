//! Fail-closed domain adapter around official `silicon-iam-client` 1.2.
//!
//! IAM publishes token-introspection and OBO verification contracts.
//! This adapter keeps those wire types isolated from Briefcase domain types and
//! cross-binds every security-relevant claim before constructing authority:
//!
//! - bearer introspection is `POST` form data containing `token` and
//!   `token_type_hint=access_token`, authenticated with Briefcase application
//!   HTTP Basic credentials and scoped with `X-Org-ID`;
//! - the mandatory unversioned compatibility handshake completes before any
//!   versioned request and every later request carries the negotiated major;
//! - introspection returns current identity, role, tags and membership version;
//!   the online snapshot is authoritative without waiting for webhooks;
//! - OBO verification submits the exact method, registered path, and body
//!   digest of the request Briefcase actually received, authenticated with
//!   application HTTP Basic credentials alone. It accepts no organization
//!   header and no idempotency key, and it is never retried: IAM consumes the
//!   proof exactly once, so a retry is indistinguishable from a replay;
//! - OBO returns scope-limited authority; role and membership disclosure scopes
//!   are required before accepting a complete snapshot;
//! - unknown response fields are ignored for forward compatibility, while any
//!   missing security-relevant field on a successful response fails closed.

use std::fmt;

use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    config::IamSettings,
    domain::actor::{
        ActorId, ActorKind, ActorRef, ApplicationId, OrganizationId,
        is_canonical_iam_application_id, is_canonical_iam_organization_id,
    },
    error::AppError,
};

const MAX_IDENTIFIER_BYTES: usize = 255;
const MAX_RESOURCE_BYTES: usize = 2_048;
const API_VERSION: &str = "v1";

/// Online IAM verifier with bounded transport and response budgets.
#[derive(Clone)]
pub struct IamClient {
    client: silicon_iam_client::Client,
    service_app_id: ApplicationId,
    service_app_secret: SecretString,
    max_response_bytes: usize,
}

/// Complete IAM identity for making an ordinary request inside one test plane.
///
/// Plane selection and application authentication are intentionally held
/// together. Supplying this value makes a request use all three test-plane
/// values; no field can silently fall back to Briefcase's production
/// application credential.
#[derive(Clone)]
pub struct IamEnvironmentCredential {
    environment_id: Option<Uuid>,
    environment_key: SecretString,
    app_id: ApplicationId,
    app_secret: SecretString,
}

impl IamEnvironmentCredential {
    /// Validates a test environment root key and its test-only Application
    /// credential.
    ///
    /// # Errors
    ///
    /// Returns [`IamClientBuildError::InvalidIdentifier`] unless the key is
    /// exactly 32 alphanumeric ASCII characters, the Application ID is
    /// canonical, and the Application secret has IAM's fixed `ask_` form.
    pub fn new(
        environment_key: SecretString,
        app_id: String,
        app_secret: SecretString,
    ) -> Result<Self, IamClientBuildError> {
        if !valid_environment_key(environment_key.expose_secret())
            || !is_canonical_iam_application_id(&app_id)
            || !valid_fixed_iam_secret(app_secret.expose_secret(), "ask_")
        {
            return Err(IamClientBuildError::InvalidIdentifier);
        }
        let app_id =
            ApplicationId::new(app_id).map_err(|_| IamClientBuildError::InvalidIdentifier)?;
        Ok(Self {
            environment_id: None,
            environment_key,
            app_id,
            app_secret,
        })
    }

    /// Binds authorization snapshots to the paired public IAM environment UUID.
    #[must_use]
    pub const fn with_environment_id(mut self, environment_id: Uuid) -> Self {
        self.environment_id = Some(environment_id);
        self
    }
}

impl fmt::Debug for IamEnvironmentCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IamEnvironmentCredential")
            .field("environment_key", &"<redacted>")
            .field("app_id", &self.app_id)
            .field("app_secret", &"<redacted>")
            .finish()
    }
}

/// Access and rotating refresh credentials returned by an IAM Application
/// login or refresh.
#[derive(Clone)]
pub struct IamApplicationTokens {
    access_token: SecretString,
    refresh_token: SecretString,
    expires_in_seconds: u64,
    scope: String,
    principal_id: Uuid,
    actor: ActorRef,
    organization_id: Option<OrganizationId>,
    idempotency_replayed: Option<bool>,
}

impl IamApplicationTokens {
    /// Returns the opaque 30-minute IAM Application access token.
    #[must_use]
    pub const fn access_token(&self) -> &SecretString {
        &self.access_token
    }

    /// Returns the rotating IAM Application refresh token.
    #[must_use]
    pub const fn refresh_token(&self) -> &SecretString {
        &self.refresh_token
    }

    /// Returns the advertised access-token lifetime in seconds.
    #[must_use]
    pub const fn expires_in_seconds(&self) -> u64 {
        self.expires_in_seconds
    }

    /// Returns the canonical, space-separated IAM scope set.
    #[must_use]
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Returns the immutable IAM principal UUID represented by the login.
    #[must_use]
    pub const fn principal_id(&self) -> Uuid {
        self.principal_id
    }

    /// Returns the represented Carbon or Silicon.
    #[must_use]
    pub const fn actor(&self) -> &ActorRef {
        &self.actor
    }

    /// Returns the optional organization selected during IAM login.
    #[must_use]
    pub const fn organization_id(&self) -> Option<&OrganizationId> {
        self.organization_id.as_ref()
    }

    /// Reports replay metadata when available; the official SDK returns none.
    #[must_use]
    pub const fn idempotency_replayed(&self) -> Option<bool> {
        self.idempotency_replayed
    }
}

impl fmt::Debug for IamApplicationTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IamApplicationTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_in_seconds", &self.expires_in_seconds)
            .field("scope", &self.scope)
            .field("principal_id", &self.principal_id)
            .field("actor", &self.actor)
            .field("organization_id", &self.organization_id)
            .field("idempotency_replayed", &self.idempotency_replayed)
            .finish()
    }
}

/// IAM token identity accepted after all current-state bindings were checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIdentity {
    authorization: Option<crate::domain::actor::RequestAuthContext>,
    principal_id: Uuid,
    actor_kind: ActorKind,
    organization_id: OrganizationId,
    membership_id: Uuid,
    authorization_epoch: i64,
    expires_at: OffsetDateTime,
}

impl VerifiedIdentity {
    /// Returns current authority obtained from the official IAM snapshot.
    #[must_use]
    pub const fn authorization(&self) -> Option<&crate::domain::actor::RequestAuthContext> {
        self.authorization.as_ref()
    }
    /// Returns the immutable principal UUID represented by the token.
    #[must_use]
    pub const fn principal_id(&self) -> Uuid {
        self.principal_id
    }

    /// Returns the represented actor category.
    #[must_use]
    pub const fn actor_kind(&self) -> ActorKind {
        self.actor_kind
    }

    /// Returns the exact organization bound to the token.
    #[must_use]
    pub const fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    /// Returns the current membership UUID bound to the token.
    #[must_use]
    pub const fn membership_id(&self) -> Uuid {
        self.membership_id
    }

    /// Returns the membership authorization epoch checked online by IAM.
    #[must_use]
    pub const fn authorization_epoch(&self) -> i64 {
        self.authorization_epoch
    }

    /// Returns the credential expiry checked by the adapter.
    #[must_use]
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }
}

/// IAM client construction failure.
#[derive(Debug, Error)]
pub enum IamClientBuildError {
    /// An IAM identifier is not representable as a domain identifier.
    #[error("invalid IAM client identifier configuration")]
    InvalidIdentifier,
    /// IAM could not negotiate the contract required by this deployment.
    #[error("failed to negotiate a compatible IAM API version")]
    Handshake(#[source] IamClientError),
}

/// A redacted online IAM verification failure.
#[derive(Debug, Error)]
pub enum IamClientError {
    /// IAM explicitly rejected or reported an inactive credential.
    #[error("IAM rejected the credential")]
    Rejected,
    /// A successful response does not match the requested security boundary.
    #[error("IAM response failed a required binding: {binding}")]
    BindingMismatch {
        /// Static binding label safe for logs.
        binding: &'static str,
    },
    /// IAM could not be reached or did not accept Briefcase service identity.
    #[error("IAM is unavailable: {reason}")]
    Unavailable {
        /// Static failure class safe for logs.
        reason: &'static str,
    },
    /// IAM returned an incomplete, oversized, or malformed success response.
    #[error("IAM returned an invalid response: {reason}")]
    InvalidResponse {
        /// Static protocol failure class safe for logs.
        reason: &'static str,
    },
}

mod official;
fn valid_server_version_catalog(versions: &[String]) -> bool {
    if versions.is_empty() || versions.len() > 16 {
        return false;
    }
    let parsed = versions
        .iter()
        .map(|version| parse_api_version(version))
        .collect::<Option<Vec<_>>>();
    parsed.is_some_and(|numbers| {
        numbers.windows(2).all(|pair| pair[0] > pair[1])
            && versions.iter().any(|version| version == API_VERSION)
    })
}

fn parse_api_version(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('v')?;
    if digits.is_empty()
        || digits.len() > 9
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

/// The exact downstream request an OBO proof is bound to.
#[derive(Clone, Copy, Debug)]
pub struct OboRequestBinding<'a> {
    /// Canonical uppercase HTTP method.
    pub method: &'a str,
    /// Registered absolute endpoint path.
    pub path: &'a str,
    /// Lowercase hexadecimal SHA-256 digest of the exact body bytes.
    pub body_sha256: &'a str,
}

/// A consumed OBO proof and everything it authorizes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedOboAccess {
    /// Current authority limited to this exact verified delegated request.
    pub authorization: Option<crate::domain::actor::RequestAuthContext>,
    /// IAM identifier of the consumed proof, unique to this one request.
    pub proof_id: Uuid,
    /// Represented Carbon or Silicon.
    pub actor: ActorRef,
    /// Organization the proof is bound to.
    pub organization_id: OrganizationId,
    /// Application that obtained the proof.
    pub issuer: ApplicationId,
    /// Registered endpoint identifier the proof was minted for.
    pub endpoint_id: String,
    /// Exact metadata bound into the proof.
    pub metadata: serde_json::Value,
}

impl From<IamClientError> for AppError {
    fn from(error: IamClientError) -> Self {
        match error {
            IamClientError::Rejected => Self::Unauthenticated,
            IamClientError::BindingMismatch { .. } => Self::Forbidden,
            IamClientError::Unavailable { .. } | IamClientError::InvalidResponse { .. } => {
                Self::DependencyUnavailable { dependency: "iam" }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireActor {
    principal_id: Uuid,
    #[serde(rename = "type")]
    kind: ActorKind,
    public_id: String,
}

#[derive(Debug, Deserialize)]
struct WireApplicationTokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: i64,
    scope: String,
    actor: WireActor,
    #[serde(default)]
    org_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireIntrospectionResponse {
    active: bool,
    #[serde(default)]
    principal_id: Option<Uuid>,
    #[serde(default)]
    actor_type: Option<ActorKind>,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    membership_id: Option<Uuid>,
    #[serde(default)]
    session_id: Option<Uuid>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    audience: Option<String>,
    #[serde(default)]
    authorization_epoch: Option<i64>,
    #[serde(default)]
    issued_at: Option<i64>,
    #[serde(default)]
    expires_at: Option<i64>,
}

impl WireIntrospectionResponse {
    fn has_metadata(&self) -> bool {
        self.principal_id.is_some()
            || self.actor_type.is_some()
            || self.org_id.is_some()
            || self.membership_id.is_some()
            || self.session_id.is_some()
            || self.scope.is_some()
            || self.client_id.is_some()
            || self.audience.is_some()
            || self.authorization_epoch.is_some()
            || self.issued_at.is_some()
            || self.expires_at.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct WireOboEndpoint {
    endpoint_id: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct WireOboResponse {
    valid: bool,
    #[serde(default)]
    proof_id: Option<Uuid>,
    #[serde(default)]
    actor: Option<WireActor>,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    issuer_app_id: Option<String>,
    #[serde(default)]
    audience: Option<String>,
    #[serde(default)]
    endpoint: Option<WireOboEndpoint>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    consumed_at: Option<OffsetDateTime>,
}

fn validate_application_tokens(
    wire: WireApplicationTokenResponse,
    idempotency_replayed: Option<bool>,
) -> Result<IamApplicationTokens, IamClientError> {
    if wire.token_type != "Bearer" {
        return Err(invalid_response("token_type"));
    }
    let expires_in_seconds =
        u64::try_from(wire.expires_in).map_err(|_| invalid_response("expires_in"))?;
    if expires_in_seconds != 1_800 {
        return Err(invalid_response("expires_in"));
    }
    if !valid_fixed_iam_secret(&wire.access_token, "oat_") {
        return Err(invalid_response("access_token"));
    }
    if !valid_fixed_iam_secret(&wire.refresh_token, "ort_") {
        return Err(invalid_response("refresh_token"));
    }
    if !valid_scope_set(&wire.scope) {
        return Err(invalid_response("scope"));
    }
    if wire.actor.principal_id.is_nil() {
        return Err(invalid_response("actor.principal_id"));
    }
    let principal_id = wire.actor.principal_id;
    validate_wire_text(
        "actor.public_id",
        &wire.actor.public_id,
        MAX_IDENTIFIER_BYTES,
    )?;
    let actor_id =
        ActorId::new(wire.actor.public_id).map_err(|_| invalid_response("actor.public_id"))?;
    let organization_id = wire
        .org_id
        .map(|value| {
            if !is_canonical_iam_organization_id(&value) {
                return Err(invalid_response("org_id"));
            }
            OrganizationId::new(value).map_err(|_| invalid_response("org_id"))
        })
        .transpose()?;

    Ok(IamApplicationTokens {
        access_token: SecretString::from(wire.access_token),
        refresh_token: SecretString::from(wire.refresh_token),
        expires_in_seconds,
        scope: wire.scope,
        principal_id,
        actor: ActorRef::new(wire.actor.kind, actor_id),
        organization_id,
        idempotency_replayed,
    })
}

fn validate_introspection(
    wire: WireIntrospectionResponse,
    expected_organization: &OrganizationId,
    expected_application: &ApplicationId,
) -> Result<VerifiedIdentity, IamClientError> {
    if !wire.active {
        if wire.has_metadata() {
            return Err(invalid_response("inactive_token_metadata"));
        }
        return Err(IamClientError::Rejected);
    }
    let organization_value = required_wire(wire.org_id, "org_id")?;
    if !is_canonical_iam_organization_id(&organization_value) {
        return Err(invalid_response("org_id"));
    }
    let organization_id =
        OrganizationId::new(organization_value).map_err(|_| invalid_response("org_id"))?;
    if &organization_id != expected_organization {
        return Err(binding_mismatch("org_id"));
    }
    if required_wire(wire.client_id, "client_id")? != expected_application.as_str() {
        return Err(binding_mismatch("client_id"));
    }
    if required_wire(wire.audience, "audience")? != expected_application.as_str() {
        return Err(binding_mismatch("audience"));
    }
    let issued_at =
        OffsetDateTime::from_unix_timestamp(required_wire(wire.issued_at, "issued_at")?)
            .map_err(|_| invalid_response("issued_at"))?;
    let expires_at =
        OffsetDateTime::from_unix_timestamp(required_wire(wire.expires_at, "expires_at")?)
            .map_err(|_| invalid_response("expires_at"))?;
    if expires_at <= issued_at {
        return Err(invalid_response("token_lifetime"));
    }
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(IamClientError::Rejected);
    }

    let principal_id = required_wire(wire.principal_id, "principal_id")?;
    let membership_id = required_wire(wire.membership_id, "membership_id")?;
    let session_id = required_wire(wire.session_id, "session_id")?;
    let scope = required_wire(wire.scope, "scope")?;
    let authorization_epoch = required_wire(wire.authorization_epoch, "authorization_epoch")?;
    if principal_id.is_nil() || membership_id.is_nil() || session_id.is_nil() {
        return Err(invalid_response("token_identity"));
    }
    if !valid_scope_set(&scope) {
        return Err(invalid_response("scope"));
    }
    if authorization_epoch < 1 {
        return Err(invalid_response("authorization_epoch"));
    }
    Ok(VerifiedIdentity {
        authorization: None,
        principal_id,
        actor_kind: required_wire(wire.actor_type, "actor_type")?,
        organization_id,
        membership_id,
        authorization_epoch,
        expires_at,
    })
}

fn validate_obo(
    wire: WireOboResponse,
    expected_audience: &ApplicationId,
    presented_application: &ApplicationId,
    expected_organization: Option<&OrganizationId>,
    binding: &OboRequestBinding<'_>,
) -> Result<VerifiedOboAccess, IamClientError> {
    if !wire.valid {
        return Err(IamClientError::Rejected);
    }

    let issuer_value = required_wire(wire.issuer_app_id, "issuer_app_id")?;
    if !is_canonical_iam_application_id(&issuer_value) {
        return Err(invalid_response("issuer_app_id"));
    }
    let issuer = ApplicationId::new(issuer_value).map_err(|_| invalid_response("issuer_app_id"))?;
    if &issuer != presented_application {
        return Err(binding_mismatch("issuer_app_id"));
    }
    let audience_value = required_wire(wire.audience, "audience")?;
    if !is_canonical_iam_application_id(&audience_value) {
        return Err(invalid_response("audience"));
    }
    let audience = ApplicationId::new(audience_value).map_err(|_| invalid_response("audience"))?;
    if &audience != expected_audience {
        return Err(binding_mismatch("audience"));
    }
    let endpoint = required_wire(wire.endpoint, "endpoint")?;
    // The proof commits to the registered path. Confirming it against the path
    // Briefcase actually served keeps one endpoint's proof from being spent on
    // another.
    if endpoint.path != binding.path {
        return Err(binding_mismatch("endpoint.path"));
    }
    validate_wire_text("endpoint.endpoint_id", &endpoint.endpoint_id, 128)?;

    // IAM derives the tenant from the two applications and never accepts one
    // from the caller, so the response is authoritative. A request that still
    // declared an organization must agree with it.
    let organization_value = required_wire(wire.org_id, "org_id")?;
    if !is_canonical_iam_organization_id(&organization_value) {
        return Err(invalid_response("org_id"));
    }
    let organization_id =
        OrganizationId::new(organization_value).map_err(|_| invalid_response("org_id"))?;
    if expected_organization.is_some_and(|expected| expected != &organization_id) {
        return Err(binding_mismatch("org_id"));
    }
    // IAM consumes the proof as part of a successful verification, so it is
    // authoritative on expiry; the timestamps are only checked for coherence.
    let expires_at = required_wire(wire.expires_at, "expires_at")?;
    let consumed_at = required_wire(wire.consumed_at, "consumed_at")?;
    if expires_at < consumed_at {
        return Err(invalid_response("consumed_at"));
    }

    let actor = required_wire(wire.actor, "actor")?;
    if actor.principal_id.is_nil() {
        return Err(invalid_response("actor.principal_id"));
    }
    validate_wire_text("actor.public_id", &actor.public_id, MAX_IDENTIFIER_BYTES)?;
    let actor_id =
        ActorId::new(actor.public_id).map_err(|_| invalid_response("actor.public_id"))?;
    let metadata = wire.metadata.unwrap_or(serde_json::Value::Null);
    if !metadata.is_object() && !metadata.is_null() {
        return Err(invalid_response("metadata"));
    }

    Ok(VerifiedOboAccess {
        authorization: None,
        proof_id: required_wire(wire.proof_id, "proof_id")?,
        actor: ActorRef::new(actor.kind, actor_id),
        organization_id,
        issuer,
        endpoint_id: endpoint.endpoint_id,
        metadata,
    })
}

fn valid_scope_set(value: &str) -> bool {
    if value.is_empty() || value.len() > 2_000 {
        return false;
    }
    let scopes = value.split(' ').collect::<Vec<_>>();
    !scopes.is_empty()
        && scopes.len() <= 100
        && scopes.iter().all(|scope| valid_scope(scope))
        && scopes.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_scope(scope: &str) -> bool {
    (2..=128).contains(&scope.len())
        && scope.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || (index != 0
                    && (byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b':' | b'-')))
        })
}

fn valid_fixed_iam_secret(value: &str, prefix: &str) -> bool {
    value.len() == prefix.len() + 43
        && value.starts_with(prefix)
        && value[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_environment_key(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn validate_outbound_binding(
    binding: &'static str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), IamClientError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(IamClientError::BindingMismatch { binding });
    }
    Ok(())
}

fn validate_wire_text(
    field: &'static str,
    value: &str,
    maximum_bytes: usize,
) -> Result<(), IamClientError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(invalid_response(field));
    }
    Ok(())
}

fn required_wire<T>(value: Option<T>, field: &'static str) -> Result<T, IamClientError> {
    value.ok_or_else(|| invalid_response(field))
}

fn binding_mismatch(binding: &'static str) -> IamClientError {
    IamClientError::BindingMismatch { binding }
}

fn invalid_response(reason: &'static str) -> IamClientError {
    IamClientError::InvalidResponse { reason }
}

fn deserialize_json<T>(body: &[u8]) -> Result<T, IamClientError>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(body).map_err(|_| invalid_response("json_schema"))
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, time::Duration};

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use secrecy::{ExposeSecret as _, SecretString};
    use serde_json::json;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, body_string, header, method, path},
    };

    use crate::config::IamSettings;

    use super::{
        ApplicationId, IamClient, IamClientBuildError, IamClientError, IamEnvironmentCredential,
        OboRequestBinding, OrganizationId, WireIntrospectionResponse, WireOboResponse,
        deserialize_json, valid_server_version_catalog, validate_introspection, validate_obo,
    };

    const PRINCIPAL_ID: &str = "01990a9d-86f1-7000-8000-000000000001";
    const MEMBERSHIP_ID: &str = "01990a9d-86f1-7000-8000-000000000002";
    const SESSION_ID: &str = "01990a9d-86f1-7000-8000-000000000003";
    const IAM_APP_ID: &str = "tos>briefcase";
    const IAM_APP_SECRET: &str = "ask_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TEST_APP_ID: &str = IAM_APP_ID;
    const TEST_APP_SECRET: &str = "ask_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TEST_ENVIRONMENT_KEY: &str = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6";
    const TEST_ENVIRONMENT_ID: &str = "01990a9d-86f1-7000-8000-000000000010";
    const BEARER_TOKEN: &str = "oat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OBO_PROOF: &str = "obo_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHORT_LIVED_TOKEN: &str = "oac_ccccccccccccccccccccccccccccccccccccccccccc";
    const REFRESH_TOKEN: &str = "ort_ddddddddddddddddddddddddddddddddddddddddddd";

    fn organization() -> OrganizationId {
        OrganizationId::new("tos").unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    #[test]
    fn version_catalog_is_ordered_unique_and_forward_compatible() {
        let strings = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<String>>()
        };
        assert!(valid_server_version_catalog(&strings(&["v3", "v2", "v1"])));
        assert!(!valid_server_version_catalog(&strings(&["v1", "v2"])));
        assert!(!valid_server_version_catalog(&strings(&["v1", "v1"])));
        assert!(!valid_server_version_catalog(&strings(&["v01"])));
        assert!(!valid_server_version_catalog(&strings(&["v2"])));
    }

    fn application() -> ApplicationId {
        ApplicationId::new("tos>silicon-dm").unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    fn client_settings(server: &MockServer) -> IamSettings {
        let base_url = Url::parse(&format!("{}/", server.uri()))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        IamSettings {
            base_url,
            app_id: IAM_APP_ID.to_owned(),
            app_secret: SecretString::from(IAM_APP_SECRET.to_owned()),
            request_timeout: Duration::from_secs(2),
            max_response_bytes: NonZeroUsize::new(65_536)
                .unwrap_or_else(|| panic!("non-zero test fixture")),
        }
    }

    fn basic_authorization() -> String {
        format!(
            "Basic {}",
            STANDARD.encode(format!("{IAM_APP_ID}:{IAM_APP_SECRET}"))
        )
    }

    fn test_basic_authorization() -> String {
        format!(
            "Basic {}",
            STANDARD.encode(format!("{TEST_APP_ID}:{TEST_APP_SECRET}"))
        )
    }

    fn environment_credential() -> IamEnvironmentCredential {
        IamEnvironmentCredential::new(
            SecretString::from(TEST_ENVIRONMENT_KEY.to_owned()),
            TEST_APP_ID.to_owned(),
            SecretString::from(TEST_APP_SECRET.to_owned()),
        )
        .unwrap_or_else(|error| panic!("test fixture: {error}"))
        .with_environment_id(
            TEST_ENVIRONMENT_ID
                .parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
    }

    fn authorization_snapshot(audience: &str, testing: bool) -> serde_json::Value {
        json!({
            "principal_id": PRINCIPAL_ID,
            "actor_type": "carbon",
            "public_id": "carbon-a",
            "organization_id": "01990a9d-86f1-7000-8000-000000000099",
            "org_id": "tos",
            "membership_id": MEMBERSHIP_ID,
            "membership_version": 7,
            "authorization_epoch": 7,
            "audience": audience,
            "testing_environment_id": if testing { Some(TEST_ENVIRONMENT_ID) } else { None },
            "scopes": ["memberships.read", "profile", "roles.read"],
            "org_role": "member",
            "tags": []
        })
    }

    fn application_token_response() -> serde_json::Value {
        json!({
            "access_token": "oat_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "refresh_token": "ort_fffffffffffffffffffffffffffffffffffffffffff",
            "token_type": "Bearer",
            "expires_in": 1800,
            "scope": "briefcase.read profile",
            "actor": {
                "principal_id": PRINCIPAL_ID,
                "type": "carbon",
                "public_id": "carbon-a"
            },
            "org_id": "tos"
        })
    }

    #[test]
    fn environment_credentials_validate_and_redact_every_secret() {
        let credential = environment_credential();
        let rendered = format!("{credential:?}");
        assert!(rendered.contains(TEST_APP_ID));
        assert!(!rendered.contains(TEST_ENVIRONMENT_KEY));
        assert!(!rendered.contains(TEST_APP_SECRET));

        assert!(matches!(
            IamEnvironmentCredential::new(
                SecretString::from("short".to_owned()),
                TEST_APP_ID.to_owned(),
                SecretString::from(TEST_APP_SECRET.to_owned()),
            ),
            Err(IamClientBuildError::InvalidIdentifier)
        ));
        assert!(matches!(
            IamEnvironmentCredential::new(
                SecretString::from(TEST_ENVIRONMENT_KEY.to_owned()),
                "briefcase".to_owned(),
                SecretString::from(TEST_APP_SECRET.to_owned()),
            ),
            Err(IamClientBuildError::InvalidIdentifier)
        ));
    }

    #[tokio::test]
    async fn startup_negotiates_the_published_api_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/version"))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Silicon-IAM-API-Version", "v1")
                    .insert_header("Vary", "Silicon-IAM-Supported-API-Versions")
                    .set_body_json(json!({
                        "service": "silicon-iam",
                        "selected_api_version": "v1",
                        "supported_api_versions": ["v2", "v1"],
                        "build": "test",
                        "commit": "test"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;

        IamClient::connect(&client_settings(&server))
            .await
            .unwrap_or_else(|error| panic!("published handshake should negotiate: {error}"));
        server.verify().await;
    }

    #[tokio::test]
    async fn environment_validation_binds_the_key_id_and_test_application() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/testing-environment"))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": TEST_ENVIRONMENT_ID,
                "name": "briefcase integration",
                "description": null,
                "key_generation": 1,
                "created_at": "2026-09-04T00:00:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/application-directory/tos%3Ebriefcase"))
            .and(header("authorization", test_basic_authorization()))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "app_id": TEST_APP_ID,
                "base_url": "https://briefcase.example.test"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let environment_id = TEST_ENVIRONMENT_ID
            .parse()
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        client
            .validate_environment_credential(&environment_credential(), environment_id)
            .await
            .unwrap_or_else(|error| panic!("IAM test-plane binding should verify: {error}"));
        server.verify().await;
    }

    #[tokio::test]
    async fn environment_validation_stops_before_application_auth_on_id_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/testing-environment"))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "01990a9d-86f1-7000-8000-000000000011",
                "name": "different environment",
                "description": null,
                "key_generation": 1,
                "created_at": "2026-09-04T00:00:00Z"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let environment_id = TEST_ENVIRONMENT_ID
            .parse()
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let result = client
            .validate_environment_credential(&environment_credential(), environment_id)
            .await;

        assert!(matches!(
            result,
            Err(IamClientError::BindingMismatch {
                binding: "testing_environment.id"
            })
        ));
        server.verify().await;
    }

    #[tokio::test]
    async fn environment_validation_rejects_a_different_application_identity_before_io() {
        let server = MockServer::start().await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let environment_id = TEST_ENVIRONMENT_ID
            .parse()
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let credential = IamEnvironmentCredential::new(
            SecretString::from(TEST_ENVIRONMENT_KEY.to_owned()),
            "other>briefcase".to_owned(),
            SecretString::from(TEST_APP_SECRET.to_owned()),
        )
        .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let result = client
            .validate_environment_credential(&credential, environment_id)
            .await;

        assert!(matches!(
            result,
            Err(IamClientError::BindingMismatch {
                binding: "testing_application.app_id"
            })
        ));
        server.verify().await;
    }

    #[tokio::test]
    async fn bearer_requests_match_the_published_transport_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .and(header("authorization", basic_authorization()))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(header("x-org-id", "tos"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!(
                "token={BEARER_TOKEN}&token_type_hint=access_token"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .set_body_json(json!({
                        "active": true,
                        "principal_id": PRINCIPAL_ID,
                        "actor_type": "carbon",
                        "client_id": IAM_APP_ID,
                        "org_id": "tos",
                        "membership_id": MEMBERSHIP_ID,
                        "session_id": SESSION_ID,
                        "scope": "memberships.read profile roles.read",
                        "audience": IAM_APP_ID,
                        "authorization": authorization_snapshot(IAM_APP_ID, false),
                        "authorization_epoch": 7,
                        "issued_at": 1_700_000_000_i64,
                        "expires_at": 4_070_908_800_i64
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .introspect_bearer(
                &SecretString::from(BEARER_TOKEN.to_owned()),
                &organization(),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("published bearer exchange should verify: {error}"));

        assert_eq!(verified.principal_id().to_string(), PRINCIPAL_ID);
        assert_eq!(verified.membership_id().to_string(), MEMBERSHIP_ID);
        server.verify().await;
    }

    #[tokio::test]
    async fn bearer_introspection_uses_only_the_selected_environment_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .and(header("authorization", test_basic_authorization()))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(header("x-org-id", "tos"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!(
                "token={BEARER_TOKEN}&token_type_hint=access_token"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .insert_header("pragma", "no-cache")
                    .set_body_json(json!({
                        "active": true,
                        "principal_id": PRINCIPAL_ID,
                        "actor_type": "carbon",
                        "client_id": TEST_APP_ID,
                        "org_id": "tos",
                        "membership_id": MEMBERSHIP_ID,
                        "session_id": SESSION_ID,
                        "scope": "memberships.read profile roles.read",
                        "audience": TEST_APP_ID,
                        "authorization": authorization_snapshot(TEST_APP_ID, true),
                        "authorization_epoch": 7,
                        "issued_at": 1_700_000_000_i64,
                        "expires_at": 4_070_908_800_i64
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .introspect_bearer(
                &SecretString::from(BEARER_TOKEN.to_owned()),
                &organization(),
                Some(&environment_credential()),
            )
            .await
            .unwrap_or_else(|error| panic!("test-plane bearer should verify: {error}"));

        assert_eq!(verified.organization_id().as_str(), "tos");
        server.verify().await;
    }

    #[tokio::test]
    async fn bearer_response_without_no_store_headers_fails_closed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "active": false
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let result = client
            .introspect_bearer(
                &SecretString::from(BEARER_TOKEN.to_owned()),
                &organization(),
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(IamClientError::InvalidResponse {
                reason: "cache_headers"
            })
        ));
        server.verify().await;
    }

    #[tokio::test]
    async fn short_lived_login_uses_the_published_form_and_returns_redacted_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/app-auth/tokens"))
            .and(header("authorization", basic_authorization()))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(header("idempotency-key", "login-operation-0001"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!(
                "app_id=tos%3Ebriefcase&slt={SHORT_LIVED_TOKEN}"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .insert_header("pragma", "no-cache")
                    .set_body_json(application_token_response()),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let tokens = client
            .exchange_short_lived_token(
                &SecretString::from(SHORT_LIVED_TOKEN.to_owned()),
                "login-operation-0001",
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("published SLT exchange should succeed: {error}"));

        assert_eq!(tokens.expires_in_seconds(), 1_800);
        assert_eq!(tokens.principal_id().to_string(), PRINCIPAL_ID);
        assert_eq!(tokens.actor().id().as_str(), "carbon-a");
        assert_eq!(
            tokens.organization_id().map(OrganizationId::as_str),
            Some("tos")
        );
        let rendered = format!("{tokens:?}");
        assert!(!rendered.contains(tokens.access_token().expose_secret()));
        assert!(!rendered.contains(tokens.refresh_token().expose_secret()));
        server.verify().await;
    }

    #[tokio::test]
    async fn refresh_uses_form_encoding_and_only_the_test_plane_credential() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/app-auth/tokens"))
            .and(header("authorization", test_basic_authorization()))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(header("idempotency-key", "refresh-operation-0001"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!(
                "app_id=tos%3Ebriefcase&refresh_token={REFRESH_TOKEN}"
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .insert_header("pragma", "no-cache")
                    .insert_header("idempotency-replayed", "true")
                    .set_body_json(application_token_response()),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let tokens = client
            .refresh_application_session(
                &SecretString::from(REFRESH_TOKEN.to_owned()),
                "refresh-operation-0001",
                Some(&environment_credential()),
            )
            .await
            .unwrap_or_else(|error| panic!("published refresh should succeed: {error}"));

        assert_eq!(tokens.idempotency_replayed(), None);
        server.verify().await;
    }

    #[tokio::test]
    async fn obo_verification_sends_only_the_published_single_use_binding() {
        let server = MockServer::start().await;
        let proof = SecretString::from(OBO_PROOF.to_owned());
        let digest = "a".repeat(64);
        Mock::given(method("POST"))
            .and(path("/api/v1/obo-access/verify"))
            .and(header("authorization", basic_authorization()))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(body_json(json!({
                "access_proof": OBO_PROOF,
                "request": {
                    "method": "POST",
                    "path": "/api/v1/obo/files",
                    "body_sha256": digest,
                }
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .insert_header("pragma", "no-cache")
                    .set_body_json(json!({
                        "valid": true,
                        "proof_id": "01990a9d-86f1-7000-8000-000000000004",
                        "issuer_app_id": "tos>silicon-dm",
                        "audience": IAM_APP_ID,
                        "authorization": authorization_snapshot(IAM_APP_ID, false),
                        "actor": {
                            "principal_id": PRINCIPAL_ID,
                            "type": "carbon",
                            "public_id": "carbon-a"
                        },
                        "org_id": "tos",
                        "endpoint": {
                            "endpoint_id": "briefcase.files.create",
                            "path": "/api/v1/obo/files"
                        },
                        "metadata": { "path": "", "name": "report.pdf" },
                        "expires_at": "2099-01-01T00:00:00Z",
                        "consumed_at": "2026-08-31T12:00:00Z"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .verify_obo(
                &proof,
                &application(),
                Some(&organization()),
                &OboRequestBinding {
                    method: "POST",
                    path: "/api/v1/obo/files",
                    body_sha256: &digest,
                },
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("published OBO exchange should verify: {error}"));

        assert_eq!(verified.actor.id().as_str(), "carbon-a");
        assert_eq!(verified.endpoint_id, "briefcase.files.create");
        assert_eq!(verified.issuer.as_str(), "tos>silicon-dm");
        server.verify().await;
    }

    #[tokio::test]
    async fn obo_proof_refusals_are_not_reported_as_dependency_outages() {
        for status in [403, 409, 410, 422, 401, 429, 500] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/v1/obo-access/verify"))
                .respond_with(ResponseTemplate::new(status).set_body_json(json!({
                    "error": {"code": "rejected", "message": "Request rejected", "request_id": "fixture"}
                })))
                .expect(1)
                .mount(&server)
                .await;
            let client = IamClient::new_without_handshake(&client_settings(&server))
                .unwrap_or_else(|error| panic!("test fixture: {error}"));
            let result = client
                .verify_obo(
                    &SecretString::from(OBO_PROOF.to_owned()),
                    &application(),
                    Some(&organization()),
                    &OboRequestBinding {
                        method: "POST",
                        path: "/api/v1/obo/files",
                        body_sha256: &"a".repeat(64),
                    },
                    None,
                )
                .await;
            if matches!(status, 403 | 409 | 410 | 422) {
                assert!(
                    matches!(result, Err(IamClientError::Rejected)),
                    "status {status}"
                );
            } else {
                assert!(
                    matches!(result, Err(IamClientError::Unavailable { .. })),
                    "status {status}"
                );
            }
            server.verify().await;
        }
    }

    #[tokio::test]
    async fn obo_verification_uses_only_the_selected_environment_credential() {
        let server = MockServer::start().await;
        let digest = "a".repeat(64);
        Mock::given(method("POST"))
            .and(path("/api/v1/obo-access/verify"))
            .and(header("authorization", test_basic_authorization()))
            .and(header("x-testing-environment-key", TEST_ENVIRONMENT_KEY))
            .and(header("silicon-iam-supported-api-versions", "v1"))
            .and(body_json(json!({
                "access_proof": OBO_PROOF,
                "request": {
                    "method": "POST",
                    "path": "/api/v1/obo/files",
                    "body_sha256": digest,
                }
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("cache-control", "no-store")
                    .insert_header("pragma", "no-cache")
                    .set_body_json(json!({
                        "valid": true,
                        "proof_id": "01990a9d-86f1-7000-8000-000000000004",
                        "issuer_app_id": "tos>silicon-dm",
                        "audience": TEST_APP_ID,
                        "authorization": authorization_snapshot(TEST_APP_ID, true),
                        "actor": {
                            "principal_id": PRINCIPAL_ID,
                            "type": "carbon",
                            "public_id": "carbon-a"
                        },
                        "org_id": "tos",
                        "endpoint": {
                            "endpoint_id": "briefcase.files.create",
                            "path": "/api/v1/obo/files"
                        },
                        "metadata": { "path": "", "name": "report.pdf" },
                        "expires_at": "2099-01-01T00:00:00Z",
                        "consumed_at": "2026-08-31T12:00:00Z"
                    })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new_without_handshake(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .verify_obo(
                &SecretString::from(OBO_PROOF.to_owned()),
                &application(),
                Some(&organization()),
                &OboRequestBinding {
                    method: "POST",
                    path: "/api/v1/obo/files",
                    body_sha256: &digest,
                },
                Some(&environment_credential()),
            )
            .await
            .unwrap_or_else(|error| panic!("test-plane OBO should verify: {error}"));

        assert_eq!(verified.organization_id.as_str(), "tos");
        server.verify().await;
    }

    #[test]
    fn published_bearer_contract_returns_only_introspected_identity() {
        let introspection_body = serde_json::to_vec(&json!({
            "active": true,
            "principal_id": "01990a9d-86f1-7000-8000-000000000001",
            "actor_type": "carbon",
            "client_id": IAM_APP_ID,
            "org_id": "tos",
            "membership_id": "01990a9d-86f1-7000-8000-000000000002",
            "session_id": "01990a9d-86f1-7000-8000-000000000003",
            "scope": "briefcase.read profile",
            "audience": IAM_APP_ID,
            "authorization_epoch": 7,
            "issued_at": 1_700_000_000_i64,
            "expires_at": 4_070_908_800_i64,
            "future_iam_field": "ignored"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let introspection: WireIntrospectionResponse = deserialize_json(&introspection_body)
            .unwrap_or_else(|error| panic!("wire contract should parse: {error}"));
        let verified = validate_introspection(introspection, &organization(), &audience())
            .unwrap_or_else(|error| panic!("introspection should verify: {error}"));

        assert_eq!(verified.organization_id().as_str(), "tos");
        assert_eq!(verified.principal_id().to_string(), PRINCIPAL_ID);
    }

    #[test]
    fn inactive_introspection_may_omit_identity_fields() {
        let wire: WireIntrospectionResponse = deserialize_json(
            &serde_json::to_vec(&json!({ "active": false }))
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("inactive response should parse: {error}"));

        assert!(matches!(
            validate_introspection(wire, &organization(), &audience()),
            Err(IamClientError::Rejected)
        ));
    }

    #[test]
    fn active_introspection_without_membership_data_fails_closed() {
        let wire: WireIntrospectionResponse = deserialize_json(
            &serde_json::to_vec(&json!({
                "active": true,
                "principal_id": "01990a9d-86f1-7000-8000-000000000001",
                "actor_type": "carbon",
                "client_id": IAM_APP_ID,
                "org_id": "tos",
                "session_id": "01990a9d-86f1-7000-8000-000000000003",
                "scope": "profile",
                "audience": IAM_APP_ID,
                "authorization_epoch": 7,
                "issued_at": 1_700_000_000_i64,
                "expires_at": 4_070_908_800_i64
            }))
            .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        assert!(matches!(
            validate_introspection(wire, &organization(), &audience()),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn inactive_introspection_that_discloses_metadata_fails_closed() {
        let wire: WireIntrospectionResponse = deserialize_json(
            &serde_json::to_vec(&json!({
                "active": false,
                "principal_id": PRINCIPAL_ID
            }))
            .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("inactive response should parse: {error}"));

        assert!(matches!(
            validate_introspection(wire, &organization(), &audience()),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }

    fn obo_binding() -> OboRequestBinding<'static> {
        OboRequestBinding {
            method: "POST",
            path: "/api/v1/obo/files",
            body_sha256: "b".repeat(64).leak(),
        }
    }

    fn obo_response(overrides: &serde_json::Value) -> WireOboResponse {
        let mut body = json!({
            "valid": true,
            "proof_id": "01990a9d-86f1-7000-8000-000000000004",
            "actor": {
                "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                "type": "silicon",
                "public_id": "researcher:tos"
            },
            "org_id": "tos",
            "issuer_app_id": "tos>silicon-dm",
            "audience": IAM_APP_ID,
            "endpoint": {
                "endpoint_id": "briefcase.files.create",
                "path": "/api/v1/obo/files"
            },
            "metadata": { "path": "public/reports", "name": "q3.pdf" },
            "expires_at": "2099-01-01T00:00:00Z",
            "consumed_at": "2026-08-31T12:00:00Z"
        });
        if let (Some(target), Some(source)) = (body.as_object_mut(), overrides.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        serde_json::from_value(body).unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    fn audience() -> ApplicationId {
        ApplicationId::new(IAM_APP_ID).unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    #[test]
    fn the_published_obo_result_yields_the_represented_actor_and_metadata() {
        let verified = validate_obo(
            obo_response(&json!({})),
            &audience(),
            &application(),
            Some(&organization()),
            &obo_binding(),
        )
        .unwrap_or_else(|error| panic!("OBO contract should verify: {error}"));

        assert_eq!(verified.actor.id().as_str(), "researcher:tos");
        assert_eq!(verified.organization_id.as_str(), "tos");
        assert_eq!(verified.metadata["name"], json!("q3.pdf"));
    }

    #[test]
    fn obo_issuer_audience_endpoint_and_organization_all_fail_closed() {
        let cases = [
            (
                json!({ "issuer_app_id": "tos>different-app" }),
                "issuer_app_id",
            ),
            (json!({ "audience": "tos>someone-else" }), "audience"),
            (
                json!({
                    "endpoint": {
                        "endpoint_id": "briefcase.files.create",
                        "path": "/api/v1/obo/other"
                    }
                }),
                "endpoint.path",
            ),
            (json!({ "org_id": "other-org" }), "org_id"),
        ];

        for (overrides, binding) in cases {
            let result = validate_obo(
                obo_response(&overrides),
                &audience(),
                &application(),
                Some(&organization()),
                &obo_binding(),
            );
            assert!(
                matches!(
                    result,
                    Err(IamClientError::BindingMismatch { binding: reported }) if reported == binding
                ),
                "{binding} must fail closed"
            );
        }
    }

    #[test]
    fn an_obo_result_without_an_endpoint_or_proof_identity_fails_closed() {
        for overrides in [json!({ "endpoint": null }), json!({ "proof_id": null })] {
            assert!(matches!(
                validate_obo(
                    obo_response(&overrides),
                    &audience(),
                    &application(),
                    Some(&organization()),
                    &obo_binding(),
                ),
                Err(IamClientError::InvalidResponse { .. })
            ));
        }
    }

    #[test]
    fn obo_application_actor_is_not_a_represented_member() {
        let response = serde_json::to_vec(&json!({
            "valid": true,
            "proof_id": "01990a9d-86f1-7000-8000-000000000004",
            "actor": {
                "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                "type": "application",
                "public_id": "external-app"
            },
            "org_id": "tos",
            "issuer_app_id": "tos>silicon-dm",
            "audience": "tos>briefcase",
            "endpoint": {
                "endpoint_id": "briefcase.files.create",
                "path": "/api/v1/obo/files"
            },
            "metadata": {},
            "expires_at": "2099-01-01T00:00:00Z",
            "consumed_at": "2026-08-31T12:00:00Z"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert!(matches!(
            deserialize_json::<WireOboResponse>(&response),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }
}
