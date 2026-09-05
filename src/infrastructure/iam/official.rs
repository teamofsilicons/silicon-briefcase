//! All IAM network operations go through the official client.

use silicon_iam_client::{Client, Credential, EnvironmentKey, IdempotencyKey, Mutation, models};

use super::*;

impl IamClient {
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
            .with_credential(Credential::Anonymous)
            .environments()
            .current()
            .await
            .map_err(|error| sdk_error(error, Operation::Environment))?;
        if current.id != expected_environment_id {
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
        let response = self
            .scoped_client(environment)?
            .oauth()
            .introspect(
                &models::TokenIntrospectionRequest {
                    token: token.expose_secret().to_owned(),
                    token_type_hint: Some(
                        models::TokenIntrospectionRequestTokenTypeHint::AccessToken,
                    ),
                },
                Some(expected_organization.as_str()),
            )
            .await
            .map_err(|error| sdk_error(error, Operation::Service))?;
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
            reason: "official_client_upstream",
        },
        _ => invalid_response("official_client_contract"),
    }
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
    if !snapshot.scopes.iter().any(|scope| scope == "roles.read")
        || !snapshot
            .scopes
            .iter()
            .any(|scope| scope == "memberships.read")
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
        models::ApplicationAuthorizationActorType::Carbon => ActorKind::Carbon,
        models::ApplicationAuthorizationActorType::Silicon => ActorKind::Silicon,
        models::ApplicationAuthorizationActorType::Other(_) => {
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
            ActorId::new(snapshot.public_id)
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
