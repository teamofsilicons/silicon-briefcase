//! Fail-closed HTTP adapter for Silicon IAM.
//!
//! IAM publishes token-introspection, userinfo, and OBO verification contracts.
//! This adapter keeps those wire types isolated from Briefcase domain types and
//! cross-binds every security-relevant claim before constructing authority:
//!
//! - bearer introspection is `POST` form data containing `token` and
//!   `token_type_hint=access_token`, authenticated with Briefcase application
//!   HTTP Basic credentials and scoped with `X-Org-ID`;
//! - the original bearer is then presented to userinfo with the same
//!   organization, and principal, actor kind, membership, and organization must
//!   agree across both responses;
//! - OBO verification uses IAM's single-use request, organization, and
//!   idempotency headers and validates its singular action binding;
//! - IAM's published OBO response does not yet carry current role and tags, so
//!   the adapter accepts those only as a coordinated extension and otherwise
//!   fails closed;
//! - unknown response fields are ignored for forward compatibility, while any
//!   missing security-relevant field on a successful response fails closed.

use std::collections::BTreeSet;

use bytes::BytesMut;
use reqwest::{StatusCode, header, redirect::Policy};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::warn;
use url::Url;
use uuid::Uuid;

use crate::{
    config::IamSettings,
    domain::actor::{
        ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, OrganizationId,
        OrganizationRole, RequestAuthContext, TagName,
    },
    error::AppError,
};

const MAX_IDENTIFIER_BYTES: usize = 255;
const MAX_TAGS: usize = 256;
const MAX_RESOURCE_BYTES: usize = 2_048;
const IDEMPOTENCY_KEY_PREFIX: &str = "briefcase-obo-v1-";

/// Online IAM verifier with bounded transport and response budgets.
#[derive(Clone)]
pub struct IamClient {
    http: reqwest::Client,
    bearer_introspection_url: Url,
    bearer_userinfo_url: Url,
    obo_verification_url: Url,
    service_app_id: ApplicationId,
    service_app_secret: SecretString,
    audience: ApplicationId,
    max_response_bytes: usize,
}

/// A complete IAM identity accepted after all expected bindings were checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIdentity {
    authorization: RequestAuthContext,
    expires_at: OffsetDateTime,
}

impl VerifiedIdentity {
    /// Returns IAM-verified authorization facts.
    #[must_use]
    pub const fn authorization(&self) -> &RequestAuthContext {
        &self.authorization
    }

    /// Consumes the result and returns IAM-verified authorization facts.
    #[must_use]
    pub fn into_authorization(self) -> RequestAuthContext {
        self.authorization
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
    /// The bounded HTTP client could not be constructed.
    #[error("failed to construct IAM HTTP client")]
    HttpClient(#[source] reqwest::Error),
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

impl IamClient {
    /// Constructs the IAM verifier with redirects and ambient proxy behavior
    /// disabled, explicit deadlines, and no response decompression features.
    ///
    /// # Errors
    ///
    /// Returns a redacted error for invalid identifiers or HTTP client setup.
    pub fn new(settings: &IamSettings) -> Result<Self, IamClientBuildError> {
        let service_app_id = ApplicationId::new(settings.app_id.clone())
            .map_err(|_| IamClientBuildError::InvalidIdentifier)?;
        let audience = ApplicationId::new(settings.audience.clone())
            .map_err(|_| IamClientBuildError::InvalidIdentifier)?;
        let mut default_headers = header::HeaderMap::new();
        default_headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        default_headers.insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        let http = reqwest::Client::builder()
            .connect_timeout(settings.connect_timeout)
            .timeout(settings.request_timeout)
            .redirect(Policy::none())
            .no_proxy()
            .default_headers(default_headers)
            .user_agent(concat!("silicon-briefcase/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(IamClientBuildError::HttpClient)?;

        Ok(Self {
            http,
            bearer_introspection_url: settings.bearer_introspection_url.clone(),
            bearer_userinfo_url: settings.bearer_userinfo_url.clone(),
            obo_verification_url: settings.obo_verification_url.clone(),
            service_app_id,
            service_app_secret: settings.app_secret.clone(),
            audience,
            max_response_bytes: settings.max_response_bytes.get(),
        })
    }

    /// Introspects an opaque IAM bearer token and verifies current membership
    /// in the exact organization selected by the request.
    ///
    /// # Errors
    ///
    /// Returns [`IamClientError::Rejected`] for an inactive credential and
    /// otherwise fails closed for transport, schema, expiry, or binding errors.
    pub async fn introspect_bearer(
        &self,
        token: &SecretString,
        expected_organization: &OrganizationId,
    ) -> Result<VerifiedIdentity, IamClientError> {
        let form = [
            ("token", token.expose_secret()),
            ("token_type_hint", "access_token"),
        ];
        let response = self
            .http
            .post(self.bearer_introspection_url.clone())
            .basic_auth(
                self.service_app_id.as_str(),
                Some(self.service_app_secret.expose_secret()),
            )
            .header("X-Org-ID", expected_organization.as_str())
            .form(&form)
            .send()
            .await
            .map_err(|error| classify_transport_error(&error))?;
        let response = require_service_success(response, "bearer_introspection")?;
        let wire: WireIntrospectionResponse =
            read_json_bounded(response, self.max_response_bytes).await?;
        let introspection = validate_introspection(wire, expected_organization)?;

        let response = self
            .http
            .get(self.bearer_userinfo_url.clone())
            .bearer_auth(token.expose_secret())
            .header("X-Org-ID", expected_organization.as_str())
            .send()
            .await
            .map_err(|error| classify_transport_error(&error))?;
        let response = require_bearer_success(response, "bearer_userinfo")?;
        let userinfo: WireUserInfoResponse =
            read_json_bounded(response, self.max_response_bytes).await?;

        validate_userinfo(&introspection, userinfo, expected_organization)
    }

    /// Verifies and consumes an OBO proof, checking every Briefcase-visible
    /// issuer, audience, action, resource, organization, and expiry binding.
    ///
    /// # Errors
    ///
    /// Returns [`IamClientError::Rejected`] when IAM reports an invalid proof
    /// and otherwise fails closed for transport, schema, expiry, or mismatch.
    pub async fn verify_obo(
        &self,
        proof: &SecretString,
        presented_application: &ApplicationId,
        expected_organization: &OrganizationId,
        action: &str,
        resource: Option<&str>,
    ) -> Result<VerifiedIdentity, IamClientError> {
        validate_outbound_binding("action", action, MAX_IDENTIFIER_BYTES)?;
        if let Some(resource) = resource {
            validate_outbound_binding("resource", resource, MAX_RESOURCE_BYTES)?;
        }
        let request = WireOboRequest {
            access_proof: proof.expose_secret(),
            audience: self.audience.as_str(),
            action,
            resource,
        };
        let idempotency_key = obo_idempotency_key(proof);
        let response = self
            .http
            .post(self.obo_verification_url.clone())
            .basic_auth(
                self.service_app_id.as_str(),
                Some(self.service_app_secret.expose_secret()),
            )
            .header("X-Org-ID", expected_organization.as_str())
            .header("Idempotency-Key", idempotency_key)
            .json(&request)
            .send()
            .await
            .map_err(|error| classify_transport_error(&error))?;
        let response = require_obo_success(response, "obo_verification")?;
        let wire: WireOboResponse = read_json_bounded(response, self.max_response_bytes).await?;

        validate_obo(
            wire,
            &self.audience,
            presented_application,
            expected_organization,
            action,
            resource,
        )
    }
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
    expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct WireUserInfoResponse {
    sub: Uuid,
    actor_type: ActorKind,
    public_id: String,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    membership_id: Option<Uuid>,
    #[serde(default)]
    org_role: Option<OrganizationRole>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Serialize)]
struct WireOboRequest<'a> {
    access_proof: &'a str,
    audience: &'a str,
    action: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct WireOboResponse {
    valid: bool,
    #[serde(default)]
    actor: Option<WireActor>,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    org_role: Option<OrganizationRole>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    issuer_app_id: Option<String>,
    #[serde(default)]
    audience: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    resource: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires_at: Option<OffsetDateTime>,
}

#[derive(Debug)]
struct ActiveIntrospection {
    principal_id: Uuid,
    actor_type: ActorKind,
    organization_id: OrganizationId,
    membership_id: Uuid,
    expires_at: OffsetDateTime,
}

fn validate_introspection(
    wire: WireIntrospectionResponse,
    expected_organization: &OrganizationId,
) -> Result<ActiveIntrospection, IamClientError> {
    if !wire.active {
        return Err(IamClientError::Rejected);
    }
    let organization_id = OrganizationId::new(required_wire(wire.org_id, "org_id")?)
        .map_err(|_| invalid_response("org_id"))?;
    if &organization_id != expected_organization {
        return Err(binding_mismatch("org_id"));
    }
    let expires_at =
        OffsetDateTime::from_unix_timestamp(required_wire(wire.expires_at, "expires_at")?)
            .map_err(|_| invalid_response("expires_at"))?;
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(IamClientError::Rejected);
    }

    Ok(ActiveIntrospection {
        principal_id: required_wire(wire.principal_id, "principal_id")?,
        actor_type: required_wire(wire.actor_type, "actor_type")?,
        organization_id,
        membership_id: required_wire(wire.membership_id, "membership_id")?,
        expires_at,
    })
}

fn validate_userinfo(
    introspection: &ActiveIntrospection,
    wire: WireUserInfoResponse,
    expected_organization: &OrganizationId,
) -> Result<VerifiedIdentity, IamClientError> {
    if wire.sub != introspection.principal_id {
        return Err(binding_mismatch("principal_id"));
    }
    if wire.actor_type != introspection.actor_type {
        return Err(binding_mismatch("actor_type"));
    }
    if required_wire(wire.membership_id, "membership_id")? != introspection.membership_id {
        return Err(binding_mismatch("membership_id"));
    }
    let userinfo_org = required_wire(wire.org_id, "org_id")?;
    if userinfo_org != introspection.organization_id.as_str() {
        return Err(binding_mismatch("org_id"));
    }

    verified_identity(
        WireActor {
            principal_id: wire.sub,
            kind: wire.actor_type,
            public_id: wire.public_id,
        },
        userinfo_org,
        required_wire(wire.org_role, "org_role")?,
        required_wire(wire.tags, "tags")?,
        introspection.expires_at,
        expected_organization,
        AuthenticationMode::Bearer,
    )
}

fn validate_obo(
    wire: WireOboResponse,
    expected_audience: &ApplicationId,
    presented_application: &ApplicationId,
    expected_organization: &OrganizationId,
    action: &str,
    resource: Option<&str>,
) -> Result<VerifiedIdentity, IamClientError> {
    if !wire.valid {
        return Err(IamClientError::Rejected);
    }

    let issuer = ApplicationId::new(required_wire(wire.issuer_app_id, "issuer_app_id")?)
        .map_err(|_| invalid_response("issuer_app_id"))?;
    if &issuer != presented_application {
        return Err(binding_mismatch("issuer_app_id"));
    }
    let audience = ApplicationId::new(required_wire(wire.audience, "audience")?)
        .map_err(|_| invalid_response("audience"))?;
    if &audience != expected_audience {
        return Err(binding_mismatch("audience"));
    }
    if required_wire(wire.action, "action")? != action {
        return Err(binding_mismatch("action"));
    }
    if wire.resource.as_deref() != resource {
        return Err(binding_mismatch("resource"));
    }

    verified_identity(
        required_wire(wire.actor, "actor")?,
        required_wire(wire.org_id, "org_id")?,
        required_wire(wire.org_role, "org_role")?,
        required_wire(wire.tags, "tags")?,
        required_wire(wire.expires_at, "expires_at")?,
        expected_organization,
        AuthenticationMode::OnBehalfOf {
            application_id: issuer,
        },
    )
}

fn verified_identity(
    wire_actor: WireActor,
    organization_id: String,
    role: OrganizationRole,
    tags: Vec<String>,
    expires_at: OffsetDateTime,
    expected_organization: &OrganizationId,
    authentication: AuthenticationMode,
) -> Result<VerifiedIdentity, IamClientError> {
    if expires_at <= OffsetDateTime::now_utc() {
        return Err(IamClientError::Rejected);
    }
    if wire_actor.principal_id.is_nil() {
        return Err(invalid_response("actor.principal_id"));
    }
    validate_wire_text(
        "actor.public_id",
        &wire_actor.public_id,
        MAX_IDENTIFIER_BYTES,
    )?;
    validate_wire_text("org_id", &organization_id, MAX_IDENTIFIER_BYTES)?;
    let actor_id =
        ActorId::new(wire_actor.public_id).map_err(|_| invalid_response("actor.public_id"))?;
    let organization_id =
        OrganizationId::new(organization_id).map_err(|_| invalid_response("org_id"))?;
    if &organization_id != expected_organization {
        return Err(binding_mismatch("org_id"));
    }
    if tags.len() > MAX_TAGS {
        return Err(invalid_response("tags"));
    }
    let tags = tags
        .into_iter()
        .map(|tag| {
            validate_wire_text("tags", &tag, MAX_IDENTIFIER_BYTES)?;
            TagName::new(tag).map_err(|_| invalid_response("tags"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let authorization = RequestAuthContext::new(
        organization_id,
        ActorRef::new(wire_actor.kind, actor_id),
        role,
        tags,
        authentication,
    );

    Ok(VerifiedIdentity {
        authorization,
        expires_at,
    })
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

fn obo_idempotency_key(proof: &SecretString) -> String {
    let digest = Sha256::digest(proof.expose_secret().as_bytes());
    format!("{IDEMPOTENCY_KEY_PREFIX}{}", hex::encode(digest))
}

fn classify_transport_error(error: &reqwest::Error) -> IamClientError {
    let reason = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection"
    } else {
        "transport"
    };
    IamClientError::Unavailable { reason }
}

fn require_service_success(
    response: reqwest::Response,
    operation: &'static str,
) -> Result<reqwest::Response, IamClientError> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(response);
    }

    warn!(iam.operation = operation, %status, "IAM request was not successful");
    let reason = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        "service_authentication"
    } else if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        "upstream_status"
    } else {
        return Err(invalid_response("unexpected_status"));
    };
    Err(IamClientError::Unavailable { reason })
}

fn require_bearer_success(
    response: reqwest::Response,
    operation: &'static str,
) -> Result<reqwest::Response, IamClientError> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(response);
    }

    warn!(iam.operation = operation, %status, "IAM request was not successful");
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(IamClientError::Rejected);
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(IamClientError::Unavailable {
            reason: "upstream_status",
        });
    }
    Err(invalid_response("unexpected_status"))
}

fn require_obo_success(
    response: reqwest::Response,
    operation: &'static str,
) -> Result<reqwest::Response, IamClientError> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(response);
    }

    warn!(iam.operation = operation, %status, "IAM request was not successful");
    if matches!(
        status,
        StatusCode::CONFLICT | StatusCode::GONE | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        return Err(IamClientError::Rejected);
    }
    let reason = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        "service_authentication"
    } else if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        "upstream_status"
    } else {
        return Err(invalid_response("unexpected_status"));
    };
    Err(IamClientError::Unavailable { reason })
}

async fn read_json_bounded<T>(
    mut response: reqwest::Response,
    maximum_bytes: usize,
) -> Result<T, IamClientError>
where
    T: DeserializeOwned,
{
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(maximum_bytes).unwrap_or(u64::MAX))
    {
        return Err(invalid_response("response_too_large"));
    }
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let media_type = value.split(';').next().map(str::trim);
            matches!(media_type, Some("application/json"))
                || media_type.is_some_and(|media_type| media_type.ends_with("+json"))
        });
    if !is_json {
        return Err(invalid_response("content_type"));
    }

    let mut body = BytesMut::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| classify_transport_error(&error))?
    {
        let new_length = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| invalid_response("response_too_large"))?;
        if new_length > maximum_bytes {
            return Err(invalid_response("response_too_large"));
        }
        body.extend_from_slice(&chunk);
    }
    deserialize_json(&body.freeze())
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
    use secrecy::SecretString;
    use serde_json::json;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, body_string, header, method, path},
    };

    use crate::config::IamSettings;

    use super::{
        ApplicationId, AuthenticationMode, IamClient, IamClientError, OrganizationId,
        WireIntrospectionResponse, WireOboResponse, WireUserInfoResponse, deserialize_json,
        obo_idempotency_key, validate_introspection, validate_obo, validate_userinfo,
    };

    const PRINCIPAL_ID: &str = "01990a9d-86f1-7000-8000-000000000001";
    const MEMBERSHIP_ID: &str = "01990a9d-86f1-7000-8000-000000000002";
    const IAM_APP_ID: &str = "silicon-briefcase";
    const IAM_APP_SECRET: &str = "test-secret";
    const BEARER_TOKEN: &str = "oat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OBO_PROOF: &str = "obo_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn organization() -> OrganizationId {
        OrganizationId::new("tos").unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    fn application() -> ApplicationId {
        ApplicationId::new("silicon-dm").unwrap_or_else(|error| panic!("test fixture: {error}"))
    }

    fn client_settings(server: &MockServer) -> IamSettings {
        let base_url = Url::parse(&format!("{}/api/v1/", server.uri()))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        IamSettings {
            bearer_introspection_url: base_url
                .join("auth/tokens/introspect")
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
            bearer_userinfo_url: base_url
                .join("oauth/userinfo")
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
            obo_verification_url: base_url
                .join("obo-access/verify")
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
            base_url,
            app_id: IAM_APP_ID.to_owned(),
            app_secret: SecretString::from(IAM_APP_SECRET.to_owned()),
            audience: IAM_APP_ID.to_owned(),
            connect_timeout: Duration::from_secs(1),
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

    #[tokio::test]
    async fn bearer_requests_match_the_published_transport_contract() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/tokens/introspect"))
            .and(header("authorization", basic_authorization()))
            .and(header("x-org-id", "tos"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string(format!(
                "token={BEARER_TOKEN}&token_type_hint=access_token"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "active": true,
                "principal_id": PRINCIPAL_ID,
                "actor_type": "carbon",
                "org_id": "tos",
                "membership_id": MEMBERSHIP_ID,
                "expires_at": 4_070_908_800_i64
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/oauth/userinfo"))
            .and(header("authorization", format!("Bearer {BEARER_TOKEN}")))
            .and(header("x-org-id", "tos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sub": PRINCIPAL_ID,
                "actor_type": "carbon",
                "public_id": "carbon-a",
                "org_id": "tos",
                "membership_id": MEMBERSHIP_ID,
                "org_role": "member",
                "tags": ["finance"]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .introspect_bearer(
                &SecretString::from(BEARER_TOKEN.to_owned()),
                &organization(),
            )
            .await
            .unwrap_or_else(|error| panic!("published bearer exchange should verify: {error}"));

        assert_eq!(verified.authorization().actor().id().as_str(), "carbon-a");
        server.verify().await;
    }

    #[tokio::test]
    async fn obo_request_matches_published_headers_and_singular_binding() {
        let server = MockServer::start().await;
        let proof = SecretString::from(OBO_PROOF.to_owned());
        let idempotency_key = obo_idempotency_key(&proof);
        Mock::given(method("POST"))
            .and(path("/api/v1/obo-access/verify"))
            .and(header("authorization", basic_authorization()))
            .and(header("x-org-id", "tos"))
            .and(header("idempotency-key", idempotency_key))
            .and(body_json(json!({
                "access_proof": OBO_PROOF,
                "audience": "silicon-briefcase",
                "action": "briefcase.entry.read",
                "resource": "entry-1"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "valid": true,
                "proof_id": "01990a9d-86f1-7000-8000-000000000004",
                "issuer_app_id": "silicon-dm",
                "audience": "silicon-briefcase",
                "actor": {
                    "principal_id": PRINCIPAL_ID,
                    "type": "carbon",
                    "public_id": "carbon-a"
                },
                "org_id": "tos",
                "action": "briefcase.entry.read",
                "resource": "entry-1",
                "expires_at": "2099-01-01T00:00:00Z",
                "consumed_at": "2026-08-31T12:00:00Z",
                "org_role": "member",
                "tags": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        let client = IamClient::new(&client_settings(&server))
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = client
            .verify_obo(
                &proof,
                &application(),
                &organization(),
                "briefcase.entry.read",
                Some("entry-1"),
            )
            .await
            .unwrap_or_else(|error| panic!("published OBO exchange should verify: {error}"));

        assert_eq!(verified.authorization().actor().id().as_str(), "carbon-a");
        server.verify().await;
    }

    #[test]
    fn published_bearer_contract_cross_binds_introspection_and_userinfo() {
        let introspection_body = serde_json::to_vec(&json!({
            "active": true,
            "principal_id": "01990a9d-86f1-7000-8000-000000000001",
            "actor_type": "carbon",
            "org_id": "tos",
            "membership_id": "01990a9d-86f1-7000-8000-000000000002",
            "expires_at": 4_070_908_800_i64,
            "future_iam_field": "ignored"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let userinfo_body = serde_json::to_vec(&json!({
            "sub": "01990a9d-86f1-7000-8000-000000000001",
            "actor_type": "carbon",
            "public_id": "carbon-a",
            "org_id": "tos",
            "membership_id": "01990a9d-86f1-7000-8000-000000000002",
            "org_role": "member",
            "tags": ["finance", "leadership"]
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let introspection: WireIntrospectionResponse = deserialize_json(&introspection_body)
            .unwrap_or_else(|error| panic!("wire contract should parse: {error}"));
        let userinfo: WireUserInfoResponse = deserialize_json(&userinfo_body)
            .unwrap_or_else(|error| panic!("userinfo contract should parse: {error}"));

        let introspection = validate_introspection(introspection, &organization())
            .unwrap_or_else(|error| panic!("introspection should verify: {error}"));
        let verified = validate_userinfo(&introspection, userinfo, &organization())
            .unwrap_or_else(|error| panic!("wire contract should verify: {error}"));

        assert_eq!(verified.authorization().organization_id().as_str(), "tos");
        assert_eq!(verified.authorization().actor().id().as_str(), "carbon-a");
        assert!(matches!(
            verified.authorization().authentication(),
            AuthenticationMode::Bearer
        ));
    }

    #[test]
    fn inactive_introspection_may_omit_identity_fields() {
        let wire: WireIntrospectionResponse = deserialize_json(
            &serde_json::to_vec(&json!({ "active": false }))
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("inactive response should parse: {error}"));

        assert!(matches!(
            validate_introspection(wire, &organization()),
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
                "org_id": "tos",
                "expires_at": 4_070_908_800_i64
            }))
            .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        assert!(matches!(
            validate_introspection(wire, &organization()),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn userinfo_principal_mismatch_fails_closed() {
        let introspection: WireIntrospectionResponse = serde_json::from_value(json!({
            "active": true,
            "principal_id": "01990a9d-86f1-7000-8000-000000000001",
            "actor_type": "carbon",
            "org_id": "tos",
            "membership_id": "01990a9d-86f1-7000-8000-000000000002",
            "expires_at": 4_070_908_800_i64
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let userinfo: WireUserInfoResponse = serde_json::from_value(json!({
            "sub": "01990a9d-86f1-7000-8000-000000000003",
            "actor_type": "carbon",
            "public_id": "carbon-a",
            "org_id": "tos",
            "membership_id": "01990a9d-86f1-7000-8000-000000000002",
            "org_role": "member",
            "tags": []
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let introspection = validate_introspection(introspection, &organization())
            .unwrap_or_else(|error| panic!("introspection should verify: {error}"));

        assert!(matches!(
            validate_userinfo(&introspection, userinfo, &organization()),
            Err(IamClientError::BindingMismatch {
                binding: "principal_id"
            })
        ));
    }

    #[test]
    fn published_obo_contract_checks_every_binding_with_authorization_extension() {
        let wire: WireOboResponse = deserialize_json(
            &serde_json::to_vec(&json!({
                "valid": true,
                "actor": {
                    "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                    "type": "silicon",
                    "public_id": "researcher:tos"
                },
                "org_id": "tos",
                "org_role": "member",
                "tags": ["research"],
                "issuer_app_id": "silicon-dm",
                "audience": "silicon-briefcase",
                "action": "briefcase.file.temporary_url",
                "resource": "018f6f9e-7b62-7d6e-bf19-b0fd6a879710",
                "expires_at": "2099-01-01T00:00:00Z"
            }))
            .unwrap_or_else(|error| panic!("test fixture: {error}")),
        )
        .unwrap_or_else(|error| panic!("OBO response should parse: {error}"));
        let audience = ApplicationId::new("silicon-briefcase")
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        let verified = validate_obo(
            wire,
            &audience,
            &application(),
            &organization(),
            "briefcase.file.temporary_url",
            Some("018f6f9e-7b62-7d6e-bf19-b0fd6a879710"),
        )
        .unwrap_or_else(|error| panic!("OBO contract should verify: {error}"));

        assert_eq!(
            verified
                .authorization()
                .originating_application()
                .map(ApplicationId::as_str),
            Some("silicon-dm")
        );
    }

    #[test]
    fn obo_issuer_header_mismatch_fails_closed() {
        let wire: WireOboResponse = serde_json::from_value(json!({
            "valid": true,
            "actor": {
                "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                "type": "carbon",
                "public_id": "carbon-a"
            },
            "org_id": "tos",
            "org_role": "owner",
            "tags": [],
            "issuer_app_id": "different-app",
            "audience": "silicon-briefcase",
            "action": "briefcase.entry.read",
            "resource": null,
            "expires_at": "2099-01-01T00:00:00Z"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let audience = ApplicationId::new("silicon-briefcase")
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert!(matches!(
            validate_obo(
                wire,
                &audience,
                &application(),
                &organization(),
                "briefcase.entry.read",
                None,
            ),
            Err(IamClientError::BindingMismatch {
                binding: "issuer_app_id"
            })
        ));
    }

    #[test]
    fn published_obo_response_without_role_and_tags_fails_closed() {
        let wire: WireOboResponse = serde_json::from_value(json!({
            "valid": true,
            "actor": {
                "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                "type": "carbon",
                "public_id": "carbon-a"
            },
            "org_id": "tos",
            "issuer_app_id": "silicon-dm",
            "audience": "silicon-briefcase",
            "action": "briefcase.entry.read",
            "resource": null,
            "expires_at": "2099-01-01T00:00:00Z"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let audience = ApplicationId::new("silicon-briefcase")
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert!(matches!(
            validate_obo(
                wire,
                &audience,
                &application(),
                &organization(),
                "briefcase.entry.read",
                None,
            ),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn obo_application_actor_is_not_a_represented_member() {
        let response = serde_json::to_vec(&json!({
            "valid": true,
            "actor": {
                "principal_id": "01990a9d-86f1-7000-8000-000000000003",
                "type": "application",
                "public_id": "external-app"
            },
            "org_id": "tos",
            "org_role": "member",
            "tags": [],
            "issuer_app_id": "silicon-dm",
            "audience": "silicon-briefcase",
            "action": "briefcase.entry.read",
            "resource": null,
            "expires_at": "2099-01-01T00:00:00Z"
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert!(matches!(
            deserialize_json::<WireOboResponse>(&response),
            Err(IamClientError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn obo_idempotency_key_is_deterministic_and_does_not_expose_the_proof() {
        let proof = SecretString::from("obo_secret-proof".to_owned());
        let first = obo_idempotency_key(&proof);
        let second = obo_idempotency_key(&proof);

        assert_eq!(first, second);
        assert!(first.starts_with("briefcase-obo-v1-"));
        assert!(!first.contains("secret-proof"));
    }
}
