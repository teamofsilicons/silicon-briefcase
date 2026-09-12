//! All IAM network operations go through the official client.

use silicon_iam_client::{Client, Credential, EnvironmentKey, IdempotencyKey, Mutation, models};

use super::*;

mod directory;
mod resilience;

impl IamClient {
    pub(crate) async fn self_email(
        &self,
        token: &SecretString,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<Option<String>, IamClientError> {
        let client = self
            .scoped_client(environment)?
            .with_credential(Credential::Bearer(token.clone()));
        match client.application_reads().me().await {
            Ok(profile) => Ok(profile
                .get("email")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)),
            Err(silicon_iam_client::Error::Api(error)) if error.status == 403 => Ok(None),
            Err(error) => Err(sdk_error(error, Operation::Service)),
        }
    }
    pub(crate) async fn provision_testing_environment(
        &self,
        name: String,
        description: Option<String>,
        iam_test_key: Option<String>,
        key: &str,
    ) -> Result<models::ApplicationTestingEnvironmentCreated, IamClientError> {
        self.scoped_client(None)?
            .applications()
            .create_testing_environment(
                &models::ApplicationTestingEnvironmentCreate {
                    name,
                    description,
                    iam_test_key,
                },
                &mutation(key)?,
            )
            .await
            .map_err(|e| sdk_error(e, Operation::Environment))
    }
    /// Returns the public application ID for production or the selected test plane.
    #[must_use]
    pub fn public_application_id<'a>(
        &'a self,
        environment: Option<&'a IamEnvironmentCredential>,
    ) -> &'a str {
        self.application_identity(environment).0.as_str()
    }

    /// Inspects a session without selecting an organization or expanding grants.
    ///
    /// # Errors
    /// Rejects inactive, expired, or wrongly bound tokens and malformed IAM responses.
    pub async fn inspect_session(
        &self,
        token: &SecretString,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<IamSessionIdentity, IamClientError> {
        if !valid_fixed_iam_secret(token.expose_secret(), "oat_") {
            return Err(IamClientError::Rejected);
        }
        let client = self.scoped_client(environment)?;
        let request = models::TokenIntrospectionRequest {
            token: token.expose_secret().to_owned(),
            token_type_hint: Some(models::TokenIntrospectionRequestTokenTypeHint::AccessToken),
        };
        let response =
            resilience::introspect(|| async { client.oauth().introspect(&request, None).await })
                .await?;
        session_identity(
            self.convert(response)?,
            self.application_identity(environment).0,
            environment,
        )
    }

    /// Builds the official IAM client and negotiates its supported API.
    ///
    /// # Errors
    /// Returns a redacted configuration or compatibility failure.
    pub async fn connect(settings: &IamSettings) -> Result<Self, IamClientBuildError> {
        let client = Self::build(settings)?;
        let version = client.client.system().negotiate().await.map_err(|error| {
            IamClientBuildError::Handshake(sdk_error(error, Operation::Service))
        })?;
        if version.service != "silicon-iam"
            || version.selected_api_version != API_VERSION
            || !valid_server_version_catalog(&version.supported_api_versions)
        {
            return Err(IamClientBuildError::Handshake(invalid_response(
                "version_negotiation",
            )));
        }
        Ok(client)
    }

    fn build(settings: &IamSettings) -> Result<Self, IamClientBuildError> {
        if !is_canonical_iam_application_id(&settings.app_id)
            || !valid_fixed_iam_secret(settings.app_secret.expose_secret(), "ask_")
        {
            return Err(IamClientBuildError::InvalidIdentifier);
        }
        let client = Client::builder(settings.base_url.as_str())
            .map_err(|_| IamClientBuildError::InvalidIdentifier)?
            .timeout(settings.request_timeout)
            .auto_update(false)
            .user_agent(concat!("silicon-briefcase/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                IamClientBuildError::Handshake(sdk_error(error, Operation::Service))
            })?;
        Ok(Self {
            client,
            service_app_id: ApplicationId::new(settings.app_id.clone())
                .map_err(|_| IamClientBuildError::InvalidIdentifier)?,
            service_app_secret: settings.app_secret.clone(),
            max_response_bytes: settings.max_response_bytes.get(),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_without_handshake(
        settings: &IamSettings,
    ) -> Result<Self, IamClientBuildError> {
        Self::build(settings)
    }

    fn scoped_client(
        &self,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<Client, IamClientError> {
        let (app_id, secret) = self.application_identity(environment);
        let client = self.client.with_credential(Credential::Application {
            app_id: app_id.as_str().to_owned(),
            secret: secret.clone(),
        });
        match environment {
            Some(environment) => Ok(client.with_environment(
                EnvironmentKey::new(environment.environment_key.expose_secret().to_owned())
                    .map_err(|_| binding_mismatch("testing_environment.key"))?,
            )),
            None => Ok(client.without_environment()),
        }
    }

    fn application_identity<'a>(
        &'a self,
        environment: Option<&'a IamEnvironmentCredential>,
    ) -> (&'a ApplicationId, &'a SecretString) {
        environment.map_or((&self.service_app_id, &self.service_app_secret), |value| {
            (&value.app_id, &value.app_secret)
        })
    }

    /// Validates the root and test-only application without production fallback.
    ///
    /// # Errors
    /// Rejects mismatched or inactive environment/application credentials.
    pub async fn validate_environment_credential(
        &self,
        environment: &IamEnvironmentCredential,
        expected_environment_id: Uuid,
    ) -> Result<(), IamClientError> {
        if expected_environment_id.is_nil() || environment.app_id != self.service_app_id {
            return Err(binding_mismatch("testing_application.app_id"));
        }
        let client = self.scoped_client(Some(environment))?;
        let current = client
            .applications()
            .testing_context()
            .await
            .map_err(|error| sdk_error(error, Operation::Environment))?;
        if current.environment_id != expected_environment_id
            || current.application.app_id != self.service_app_id.as_str()
        {
            return Err(binding_mismatch("testing_environment.id"));
        }
        let application = client
            .applications()
            .discover_base_url(environment.app_id.as_str())
            .await
            .map_err(|error| sdk_error(error, Operation::Environment))?;
        if application.app_id != environment.app_id.as_str() {
            return Err(binding_mismatch("application_directory.app_id"));
        }
        Ok(())
    }

    /// Exchanges one SLT using the caller's durable retry key.
    ///
    /// # Errors
    /// Rejects invalid credentials or reports redacted upstream failures.
    pub async fn exchange_short_lived_token(
        &self,
        slt: &SecretString,
        idempotency_key: &str,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<IamApplicationTokens, IamClientError> {
        if !valid_fixed_iam_secret(slt.expose_secret(), "oac_") {
            return Err(IamClientError::Rejected);
        }
        let client = self.scoped_client(environment)?;
        let mutation = mutation(idempotency_key)?;
        let tokens = client
            .oauth()
            .login(
                self.application_identity(environment).0.as_str(),
                slt.expose_secret(),
                &mutation,
            )
            .await
            .map_err(|error| sdk_error(error, Operation::Token))?;
        validate_application_tokens(self.convert(tokens)?, None)
    }

    /// Lists only active memberships explicitly selected by the user in IAM
    /// for this Application and parent login. An empty list grants no access;
    /// new memberships are not implicitly added to existing consent.
    ///
    /// # Errors
    /// Rejects inactive tokens, mismatched audiences, malformed organization
    /// handles, and snapshots from the wrong testing environment.
    pub async fn reachable_organizations(
        &self,
        access_token: &SecretString,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<Vec<OrganizationId>, IamClientError> {
        if !valid_fixed_iam_secret(access_token.expose_secret(), "oat_") {
            return Err(IamClientError::Rejected);
        }
        let authorizations = self
            .scoped_client(environment)?
            .oauth()
            .authorizations(access_token.expose_secret())
            .await
            .map_err(|error| sdk_error(error, Operation::Service))?
            .ok_or(IamClientError::Rejected)?;
        let expected_audience = self.application_identity(environment).0.as_str();
        let expected_environment = environment.and_then(|value| value.environment_id);
        let mut organizations = Vec::with_capacity(authorizations.len());
        for authorization in authorizations {
            if authorization.audience.as_str() != expected_audience
                || authorization.testing_environment_id != expected_environment
                || !is_canonical_iam_organization_id(authorization.org_id.as_str())
                || authorization.membership_version < 1
                || authorization.authorization_epoch < 1
            {
                return Err(invalid_response("authorization.organization"));
            }
            organizations.push(
                OrganizationId::new(authorization.org_id)
                    .map_err(|_| invalid_response("authorization.org_id"))?,
            );
        }
        organizations.sort();
        organizations.dedup();
        Ok(organizations)
    }

    /// Rotates a refresh token without retrying or retaining session state.
    ///
    /// # Errors
    /// Rejects spent/revoked credentials or reports redacted upstream failures.
    pub async fn refresh_application_session(
        &self,
        refresh_token: &SecretString,
        idempotency_key: &str,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<IamApplicationTokens, IamClientError> {
        if !valid_fixed_iam_secret(refresh_token.expose_secret(), "ort_") {
            return Err(IamClientError::Rejected);
        }
        let client = self.scoped_client(environment)?;
        let tokens = client
            .oauth()
            .refresh(
                self.application_identity(environment).0.as_str(),
                refresh_token.expose_secret(),
                &mutation(idempotency_key)?,
            )
            .await
            .map_err(|error| sdk_error(error, Operation::Token))?;
        validate_application_tokens(self.convert(tokens)?, None)
    }

    /// Gets current token authority, including synchronous membership facts.
    ///
    /// # Errors
    /// Fails closed on absent, undisclosed, or mismatched authority.
    pub async fn introspect_bearer(
        &self,
        token: &SecretString,
        expected_organization: &OrganizationId,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<VerifiedIdentity, IamClientError> {
        let client = self.scoped_client(environment)?;
        let request = models::TokenIntrospectionRequest {
            token: token.expose_secret().to_owned(),
            token_type_hint: Some(models::TokenIntrospectionRequestTokenTypeHint::AccessToken),
        };
        let response = resilience::introspect(|| async {
            client
                .oauth()
                .introspect(&request, Some(expected_organization.as_str()))
                .await
        })
        .await?;
        let snapshot = response.authorization.clone();
        let scopes = response.scope.clone();
        let mut verified = validate_introspection(
            self.convert(response)?,
            expected_organization,
            self.application_identity(environment).0,
        )?;
        let snapshot = snapshot.ok_or_else(|| invalid_response("authorization_missing"))?;
        if snapshot.principal_id != verified.principal_id
            || snapshot.membership_id != verified.membership_id
            || snapshot.authorization_epoch != verified.authorization_epoch
            || snapshot.scopes.join(" ") != scopes.unwrap_or_default()
        {
            return Err(binding_mismatch("authorization.token"));
        }
        let authority = authorization(
            snapshot,
            self.application_identity(environment).0,
            expected_organization,
            environment,
            crate::domain::actor::AuthenticationMode::Bearer,
        )?;
        if authority.actor().kind() != verified.actor_kind {
            return Err(binding_mismatch("authorization.actor_type"));
        }
        verified.authorization = Some(authority);
        Ok(verified)
    }

    /// Consumes an exact-request OBO proof and its current delegated authority.
    ///
    /// # Errors
    /// Rejects spent or mismatched proofs. Never retries an uncertain verify.
    pub async fn verify_obo(
        &self,
        proof: &SecretString,
        presented_application: &ApplicationId,
        expected_organization: Option<&OrganizationId>,
        binding: &OboRequestBinding<'_>,
        environment: Option<&IamEnvironmentCredential>,
    ) -> Result<VerifiedOboAccess, IamClientError> {
        if !valid_fixed_iam_secret(proof.expose_secret(), "obo_") {
            return Err(IamClientError::Rejected);
        }
        validate_outbound_binding("obo.method", binding.method, 16)?;
        validate_outbound_binding("obo.path", binding.path, MAX_RESOURCE_BYTES)?;
        validate_outbound_binding("obo.body_sha256", binding.body_sha256, 64)?;
        let request = serde_json::from_value(serde_json::json!({"access_proof":proof.expose_secret(),"request":{"method":binding.method,"path":binding.path,"body_sha256":binding.body_sha256}}))
            .map_err(|_| binding_mismatch("obo.request"))?;
        let response = self
            .scoped_client(environment)?
            .obo()
            .verify(&request)
            .await
            .map_err(|error| sdk_error(error, Operation::Obo))?;
        let snapshot = response.authorization.clone();
        if snapshot.principal_id != response.actor.principal_id {
            return Err(binding_mismatch("authorization.principal"));
        }
        let mut verified = validate_obo(
            self.convert(response)?,
            self.application_identity(environment).0,
            presented_application,
            expected_organization,
            binding,
        )?;
        let authority = authorization(
            snapshot,
            self.application_identity(environment).0,
            &verified.organization_id,
            environment,
            crate::domain::actor::AuthenticationMode::OnBehalfOf {
                application_id: verified.issuer.clone(),
            },
        )?;
        if authority.actor() != &verified.actor {
            return Err(binding_mismatch("authorization.actor"));
        }
        verified.authorization = Some(authority);
        Ok(verified)
    }

    fn convert<T: Serialize, U: DeserializeOwned>(&self, value: T) -> Result<U, IamClientError> {
        let bytes = serde_json::to_vec(&value).map_err(|_| invalid_response("sdk_model"))?;
        if bytes.len() > self.max_response_bytes {
            return Err(invalid_response("response_size"));
        }
        deserialize_json(&bytes)
    }
}

fn mutation(key: &str) -> Result<Mutation, IamClientError> {
    IdempotencyKey::parse(key)
        .map(Mutation::with_key)
        .map_err(|_| binding_mismatch("idempotency_key"))
}

#[derive(Clone, Copy)]
enum Operation {
    Service,
    Token,
    Obo,
    Environment,
}

fn sdk_error(error: silicon_iam_client::Error, operation: Operation) -> IamClientError {
    use silicon_iam_client::Error;
    resilience::log_error(&error);
    match error {
        Error::Api(error)
            if matches!(operation, Operation::Environment)
                && matches!(error.status, 400 | 401 | 403 | 404 | 422)
                || matches!(operation, Operation::Token) && error.status == 400
                || matches!(operation, Operation::Obo)
                    && matches!(error.status, 403 | 409 | 410 | 422) =>
        {
            IamClientError::Rejected
        }
        Error::Api(_)
        | Error::RateLimited { .. }
        | Error::Transport(_)
        | Error::UnstructuredResponse { .. } => IamClientError::Unavailable {
            reason: resilience::failure_class(&error),
        },
        _ => invalid_response("official_client_contract"),
    }
}

pub(super) fn session_identity(
    response: models::TokenIntrospection,
    audience: &ApplicationId,
    environment: Option<&IamEnvironmentCredential>,
) -> Result<IamSessionIdentity, IamClientError> {
    if !response.active {
        return Err(IamClientError::Rejected);
    }
    if response.client_id.as_deref() != Some(audience.as_str())
        || response.audience.as_deref() != Some(audience.as_str())
    {
        return Err(binding_mismatch("session.audience"));
    }
    let principal_id = response
        .principal_id
        .filter(|id| !id.is_nil())
        .ok_or_else(|| invalid_response("session.principal_id"))?;
    let actor_kind = match response.actor_type {
        Some(models::TokenIntrospectionActorType::Carbon) => ActorKind::Carbon,
        Some(models::TokenIntrospectionActorType::Silicon) => ActorKind::Silicon,
        _ => return Err(invalid_response("session.actor_type")),
    };
    let expires_at = response
        .expires_at
        .ok_or_else(|| invalid_response("session.expires_at"))?;
    if expires_at <= OffsetDateTime::now_utc().unix_timestamp() {
        return Err(IamClientError::Rejected);
    }
    let snapshots = match (response.authorization, response.authorizations) {
        (Some(snapshot), None) if response.org_id.as_deref() == Some(snapshot.org_id.as_str()) => {
            vec![snapshot]
        }
        (None, Some(snapshots)) if response.org_id.is_none() => snapshots,
        _ => return Err(invalid_response("session.authorizations")),
    };
    let mut identity = IamSessionIdentity {
        principal_id,
        actor_kind,
        public_id: None,
        organizations: Vec::new(),
        expires_at,
    };
    for snapshot in snapshots {
        let snapshot_kind = match snapshot.actor_type {
            Some(models::ApplicationAuthorizationActorType::Carbon) => ActorKind::Carbon,
            Some(models::ApplicationAuthorizationActorType::Silicon) => ActorKind::Silicon,
            Some(models::ApplicationAuthorizationActorType::Other(_)) | None => {
                return Err(invalid_response("session.actor_type"));
            }
        };
        if snapshot.principal_id != principal_id
            || snapshot_kind != actor_kind
            || snapshot.audience != audience.as_str()
            || snapshot.testing_environment_id != environment.and_then(|value| value.environment_id)
            || !is_canonical_iam_organization_id(&snapshot.org_id)
            || snapshot.membership_id.is_nil()
            || snapshot.organization_id.is_nil()
            || snapshot.membership_version < 1
            || snapshot.authorization_epoch < 1
            || identity
                .public_id
                .as_ref()
                .is_some_and(|id| Some(id) != snapshot.public_id.as_ref())
        {
            return Err(binding_mismatch("session.authorization"));
        }
        ActorId::new(snapshot.public_id.clone().ok_or(IamClientError::Rejected)?)
            .map_err(|_| invalid_response("session.public_id"))?;
        identity.public_id = snapshot.public_id;
        identity.organizations.push(
            OrganizationId::new(snapshot.org_id).map_err(|_| invalid_response("session.org_id"))?,
        );
    }
    identity.organizations.sort();
    identity.organizations.dedup();
    Ok(identity)
}

fn authorization(
    snapshot: models::ApplicationAuthorization,
    audience: &ApplicationId,
    organization: &OrganizationId,
    environment: Option<&IamEnvironmentCredential>,
    authentication: crate::domain::actor::AuthenticationMode,
) -> Result<crate::domain::actor::RequestAuthContext, IamClientError> {
    use crate::domain::actor::{
        IamMembershipBinding, OrganizationRole, RequestAuthContext, TagName,
    };
    if snapshot.audience != audience.as_str()
        || snapshot.org_id != organization.as_str()
        || snapshot.principal_id.is_nil()
        || snapshot.organization_id.is_nil()
        || snapshot.membership_id.is_nil()
        || snapshot.membership_version < 1
        || snapshot.authorization_epoch < 1
        || snapshot.testing_environment_id != environment.and_then(|value| value.environment_id)
        || environment.is_some_and(|value| value.environment_id.is_none())
        || !valid_scope_set(&snapshot.scopes.join(" "))
    {
        return Err(binding_mismatch("authorization.scope"));
    }
    if !snapshot
        .scopes
        .iter()
        .any(|scope| scope == "self.membership.read")
        || !snapshot
            .scopes
            .iter()
            .any(|scope| scope == "self.identity.read")
    {
        return Err(IamClientError::Rejected);
    }
    let role = match snapshot.org_role.as_deref() {
        Some("owner") => OrganizationRole::Owner,
        Some("admin") => OrganizationRole::Admin,
        Some("member") => OrganizationRole::Member,
        _ => return Err(IamClientError::Rejected),
    };
    let actor_kind = match snapshot.actor_type {
        Some(models::ApplicationAuthorizationActorType::Carbon) => ActorKind::Carbon,
        Some(models::ApplicationAuthorizationActorType::Silicon) => ActorKind::Silicon,
        Some(models::ApplicationAuthorizationActorType::Other(_)) | None => {
            return Err(invalid_response("authorization.actor_type"));
        }
    };
    let mut tags = Vec::new();
    for tag in snapshot.tags.ok_or(IamClientError::Rejected)? {
        if tag.id.is_nil()
            || tags
                .iter()
                .any(|(id, name): &(Uuid, TagName)| *id == tag.id || name.as_str() == tag.name)
        {
            return Err(invalid_response("authorization.tag_id"));
        }
        tags.push((
            tag.id,
            TagName::new(tag.name).map_err(|_| invalid_response("authorization.tag_name"))?,
        ));
    }
    let context = RequestAuthContext::new(
        organization.clone(),
        ActorRef::new(
            actor_kind,
            ActorId::new(snapshot.public_id.ok_or(IamClientError::Rejected)?)
                .map_err(|_| invalid_response("authorization.public_id"))?,
        ),
        role,
        tags.iter().map(|(_, name)| name.clone()),
        authentication,
    );
    Ok(context.with_iam_binding(IamMembershipBinding {
        organization_id: snapshot.organization_id,
        principal_id: snapshot.principal_id,
        membership_id: snapshot.membership_id,
        membership_version: snapshot.membership_version,
        authorization_epoch: snapshot.authorization_epoch,
        tags,
    }))
}
