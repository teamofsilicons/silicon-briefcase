//! Silicon Accounts adapter built on the official `silicon-accounts-client`.
//!
//! - Access tokens are `EdDSA` JWTs verified locally against the cached JWKS:
//!   signature, `exp`/`nbf`, `aud` = Briefcase's app id and `iss` = the
//!   Silicon Accounts public URL. An unknown `kid` refetches the keys at most
//!   every 30 seconds.
//! - Every bearer request is introspected online. Only active access tokens
//!   issued to Briefcase are accepted; authorization answers are never cached.
//! - Proofs (`sap_…`) are verified with Silicon Accounts on every use.
//! - Account lookups spend a local budget kept inside Silicon Accounts' 600
//!   lookups a minute per app; callers consult the account cache first.

use std::{sync::Arc, time::Duration};

use secrecy::{ExposeSecret as _, SecretString};
use silicon_accounts_client::{
    AccountSummary, AccountsClient, AppClient, Claims, Error as ClientError, Jwks,
    ProofVerification, TokenError, UserInfo, VerifyOptions, verify_access_token,
};
use thiserror::Error;

use crate::{config::AccountsSettings, error::AppError};

mod budget;
mod directory;
mod jwks;
#[cfg(test)]
pub(crate) mod test_support;

pub use budget::{ExpiringMap, LOOKUPS_PER_MINUTE, TokenBucket};
pub use directory::{AccountDirectory, DriveOwner};
pub use jwks::{JwksCache, MAX_AGE as JWKS_MAX_AGE, UNKNOWN_KEY_REFETCH_INTERVAL};

/// A failure talking to Silicon Accounts, with a reason safe to show callers.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AccountsError {
    /// The access token failed verification.
    #[error("{message}")]
    InvalidToken {
        /// Stable code from the client, such as `token_expired`.
        code: &'static str,
        /// What is wrong with the token and what to do.
        message: String,
    },
    /// Silicon Accounts refused Briefcase's own app credentials.
    #[error("Silicon Accounts refused Briefcase's app credentials ({code}): {message}")]
    CredentialsRejected {
        /// The service's error code.
        code: String,
        /// The service's message.
        message: String,
    },
    /// Silicon Accounts asked Briefcase to slow down, or Briefcase's own
    /// lookup budget is spent.
    #[error("Silicon Accounts lookups are rate limited; retry in {retry_after_seconds} s")]
    RateLimited {
        /// Seconds until a retry can succeed.
        retry_after_seconds: u64,
    },
    /// Silicon Accounts could not be reached or answered unexpectedly.
    #[error("Silicon Accounts is unavailable ({code}): {message}")]
    Unavailable {
        /// Client or service error code.
        code: String,
        /// What failed.
        message: String,
    },
}

impl From<AccountsError> for AppError {
    fn from(error: AccountsError) -> Self {
        match error {
            AccountsError::InvalidToken { code, message } => Self::unauthorized(code, message),
            AccountsError::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            AccountsError::CredentialsRejected { code, message } => {
                tracing::error!(
                    event = "accounts_credentials_rejected",
                    error_code = %code,
                    error_message = %message,
                    "Silicon Accounts refused Briefcase's app credentials; check BRIEFCASE_APP_ID and BRIEFCASE_APP_SECRET"
                );
                Self::DependencyUnavailable {
                    dependency: "silicon_accounts",
                }
            }
            AccountsError::Unavailable { code, message } => {
                tracing::warn!(
                    event = "accounts_unavailable",
                    error_code = %code,
                    error_message = %message,
                    "Silicon Accounts call failed"
                );
                Self::DependencyUnavailable {
                    dependency: "silicon_accounts",
                }
            }
        }
    }
}

/// What introspection said about one token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Introspection {
    /// The token is active for this account uuid.
    Active {
        /// The token's `sub`.
        subject: Option<String>,
    },
    /// The token is revoked, expired, or not Briefcase's.
    Inactive,
}

/// The Silicon Accounts calls Briefcase makes, with their caches and budgets.
pub struct AccountsGateway {
    client: AccountsClient,
    app_id: String,
    app_secret: SecretString,
    issuer: String,
    jwks: JwksCache,
    lookups: TokenBucket,
}

impl std::fmt::Debug for AccountsGateway {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountsGateway")
            .field("api_url", &self.client.base_url().as_str())
            .field("app_id", &self.app_id)
            .field("issuer", &self.issuer)
            .field("jwks", &self.jwks)
            .finish_non_exhaustive()
    }
}

impl AccountsGateway {
    /// Builds the gateway. No request is sent until the first verification.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::Unavailable`] when the client cannot be built
    /// for the configured URL.
    pub fn new(settings: &AccountsSettings) -> Result<Self, AccountsError> {
        let client = AccountsClient::builder()
            .base_url(settings.api_url.as_str())
            .timeout(settings.request_timeout)
            .connect_timeout(settings.request_timeout.min(Duration::from_secs(5)))
            .user_agent(format!("silicon-briefcase/{}", env!("CARGO_PKG_VERSION")))
            // Briefcase records no telemetry and opts out of Silicon Accounts'.
            .telemetry(false)
            .build()
            .map_err(|error| unavailable(&error))?;
        Ok(Self {
            client,
            app_id: settings.app_id.clone(),
            app_secret: settings.app_secret.clone(),
            issuer: settings.issuer.clone(),
            jwks: JwksCache::default(),
            lookups: TokenBucket::per_minute(LOOKUPS_PER_MINUTE),
        })
    }

    /// Starts from a known key set instead of fetching one (tests, warm starts).
    #[must_use]
    pub fn with_jwks(mut self, jwks: Jwks) -> Self {
        self.jwks = JwksCache::with_keys(jwks);
        self
    }

    /// Briefcase's app id: the audience every accepted token carries.
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// The exact issuer every accepted token carries.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The server-to-server Silicon Accounts URL.
    #[must_use]
    pub fn api_url(&self) -> &str {
        self.client.base_url().as_str()
    }

    fn app(&self) -> AppClient<'_> {
        self.client.as_app(
            self.app_id.clone(),
            self.app_secret.expose_secret().to_owned(),
        )
    }

    /// Verifies an access token locally.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::InvalidToken`] with the precise reason when
    /// the token is malformed, forged, expired, not yet valid, issued to
    /// another app or by another issuer, or signed by an unknown key; and
    /// [`AccountsError::Unavailable`] when no key set could ever be fetched.
    pub async fn verify_access_token(&self, token: &str) -> Result<Claims, AccountsError> {
        let mut options = VerifyOptions::for_app(&self.app_id).with_issuer(self.issuer.clone());
        options.leeway_seconds = 0;
        let jwks: Arc<Jwks> = self
            .jwks
            .get(&self.client)
            .await
            .map_err(|error| unavailable(&error))?;
        match verify_access_token(&jwks, token, &options) {
            Err(ClientError::Token(TokenError::UnknownKey { .. })) => {
                match self.jwks.refetch_for_unknown_key(&self.client).await {
                    Some(fresh) => verify_access_token(&fresh, token, &options).map_err(token_error),
                    None => Err(AccountsError::InvalidToken {
                        code: "token_unknown_key",
                        message: "The access token is signed with a key Silicon Accounts does not publish. Sign in again; a token that still fails was not issued by this Silicon Accounts.".to_owned(),
                    }),
                }
            }
            result => result.map_err(token_error),
        }
    }

    /// Confirms the token is an active access token for Briefcase now.
    /// Refresh tokens and missing credential/audience facts fail closed.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::Unavailable`] or
    /// [`AccountsError::CredentialsRejected`] when the check itself failed.
    pub async fn introspect(&self, token: &str) -> Result<Introspection, AccountsError> {
        let answer = match self.app().introspect(token).await {
            Ok(result)
                if result.active
                    && result.token_type.as_deref() == Some("access_token")
                    && result.client_id.as_deref() == Some(self.app_id.as_str()) =>
            {
                Introspection::Active {
                    subject: result.sub,
                }
            }
            Ok(_) => Introspection::Inactive,
            Err(error) => return Err(service_error(&error)),
        };
        Ok(answer)
    }

    /// Verifies a proof token another app sent.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::Unavailable`] or
    /// [`AccountsError::CredentialsRejected`] when the check itself failed.
    /// An invalid proof is `Ok(ProofVerification::Invalid)`.
    pub async fn verify_proof(&self, token: &str) -> Result<ProofVerification, AccountsError> {
        self.app()
            .verify_proof(token)
            .await
            .map_err(|error| service_error(&error))
    }

    /// Issues a short-lived, address-book-only proof after Briefcase records consent.
    pub(crate) async fn contacts_proof(
        &self,
        token: &str,
        write: bool,
    ) -> Result<SecretString, AppError> {
        let proof = self
            .app()
            .issue_user_verification(
                &silicon_accounts_client::IssueUserVerification {
                    subject_token: token.to_owned(),
                    receiving_app: "silicon-contacts".into(),
                    scopes: vec![
                        if write {
                            "contacts.write"
                        } else {
                            "contacts.read"
                        }
                        .into(),
                    ],
                    access_ttl_seconds: Some(60),
                },
                None,
            )
            .await
            .map_err(|error| AppError::from(service_error(&error)))?;
        Ok(SecretString::from(proof.proof_token.expose().to_owned()))
    }

    /// A short-lived proof for one incoming-share check. It carries no address-book access.
    pub(crate) async fn share_check_proof(
        &self,
        recipient: &str,
        sender: &str,
    ) -> Result<SecretString, AppError> {
        let proof = self
            .app()
            .issue_app_verification(
                &silicon_accounts_client::IssueAppVerification {
                    receiving_app: "silicon-contacts".into(),
                    scopes: vec![
                        "briefcase.share.check".into(),
                        format!("recipient:{recipient}"),
                        format!("sender:{sender}"),
                    ],
                    access_ttl_seconds: Some(60),
                },
                None,
            )
            .await
            .map_err(|error| AppError::from(service_error(&error)))?;
        Ok(SecretString::from(proof.proof_token.expose().to_owned()))
    }

    /// Uses only profile fields an active app member has shared with Briefcase.
    pub(crate) async fn recipient_profile(
        &self,
        uuid: &str,
    ) -> Result<Option<(String, Option<String>)>, AccountsError> {
        self.spend_lookup()?;
        match self.app().user(uuid).await {
            Ok(member)
                if member.uuid == uuid
                    && member.status == "active"
                    && member.granted_scopes.iter().any(|scope| scope == "profile") =>
            {
                Ok(Some((member.display_name, member.pfp_url)))
            }
            Ok(_) => Ok(None),
            Err(error) if error.status() == Some(404) => Ok(None),
            Err(error) => Err(service_error(&error)),
        }
    }

    /// Looks an account up by uuid. `Ok(None)` when no such account exists.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::RateLimited`] when the lookup budget is spent,
    /// or a service error.
    pub async fn lookup(&self, uuid: &str) -> Result<Option<AccountSummary>, AccountsError> {
        self.spend_lookup()?;
        not_found_as_none(self.app().lookup(uuid).await)
    }

    /// Looks an account up by its current `c:`/`si:` id. `Ok(None)` when no
    /// account has that id now.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::RateLimited`] when the lookup budget is spent,
    /// or a service error.
    pub async fn lookup_by_id(&self, id: &str) -> Result<Option<AccountSummary>, AccountsError> {
        self.spend_lookup()?;
        not_found_as_none(self.app().lookup_by_id(id).await)
    }

    /// Reads what the account shares with Briefcase, with its own token.
    ///
    /// # Errors
    ///
    /// Returns [`AccountsError::InvalidToken`] when Silicon Accounts refuses
    /// the token, or a service error.
    pub async fn userinfo(&self, access_token: &str) -> Result<UserInfo, AccountsError> {
        self.app()
            .userinfo(access_token)
            .await
            .map_err(|error| match error.status() {
                Some(401) => AccountsError::InvalidToken {
                    code: "token_rejected",
                    message: format!(
                        "Silicon Accounts refused the access token: {} Sign in again.",
                        error.message()
                    ),
                },
                _ => service_error(&error),
            })
    }

    fn spend_lookup(&self) -> Result<(), AccountsError> {
        self.lookups
            .try_take()
            .map_err(|wait| AccountsError::RateLimited {
                retry_after_seconds: wait.as_secs().max(1),
            })
    }
}

fn not_found_as_none(
    result: silicon_accounts_client::Result<AccountSummary>,
) -> Result<Option<AccountSummary>, AccountsError> {
    match result {
        Ok(summary) => Ok(Some(summary)),
        Err(error) if error.status() == Some(404) => Ok(None),
        Err(error) => Err(service_error(&error)),
    }
}

/// Describes a rejected access token to the caller (the client library's
/// hints are written for the verifying app, not for the caller).
fn token_error(error: ClientError) -> AccountsError {
    let ClientError::Token(token) = error else {
        return unavailable(&error);
    };
    let message = match &token {
        TokenError::Malformed(_) => format!(
            "{} Send the access_token from your Silicon Accounts sign-in, not a refresh token, short-lived token or proof.",
            token.message()
        ),
        TokenError::MissingClaim(_) => format!(
            "{} Sign in again with Silicon Accounts.",
            token.message()
        ),
        TokenError::UnsupportedAlgorithm(_)
        | TokenError::BadSignature
        | TokenError::InvalidKey(_) => "The access token's signature does not verify with Silicon Accounts' published keys. Sign in again with Silicon Accounts.".to_owned(),
        TokenError::UnknownKey { .. } => "The access token is signed with a key Silicon Accounts does not publish. Sign in again; a token that still fails was not issued by this Silicon Accounts.".to_owned(),
        TokenError::Expired { .. } => format!(
            "{} Refresh it with the refresh token and retry.",
            token.message()
        ),
        TokenError::NotYetValid => "The access token is not valid yet (its nbf is in the future). Check the clock of the machine that obtained it, or retry in a few seconds.".to_owned(),
        TokenError::WrongAudience { .. } => "The access token was issued to another app. Briefcase accepts only access tokens issued to briefcase; an app acting for an account sends a User verification proof to the /api/v1/obo/ routes instead.".to_owned(),
        TokenError::WrongIssuer { expected } => format!(
            "The access token was not issued by the Silicon Accounts Briefcase trusts ({expected}). Sign in at that Silicon Accounts."
        ),
        other => format!("{} Sign in again with Silicon Accounts.", other.message()),
    };
    AccountsError::InvalidToken {
        code: token.code(),
        message,
    }
}

fn service_error(error: &ClientError) -> AccountsError {
    match (error.status(), error.code()) {
        (Some(401 | 403), code)
            if matches!(
                code,
                "invalid_app_credentials"
                    | "app_credentials_required"
                    | "app_disabled"
                    | "invalid_client"
            ) =>
        {
            AccountsError::CredentialsRejected {
                code: code.to_owned(),
                message: error.message(),
            }
        }
        (Some(429), _) => AccountsError::RateLimited {
            retry_after_seconds: error.retry_after().map_or(1, |wait| wait.as_secs().max(1)),
        },
        _ => unavailable(error),
    }
}

fn unavailable(error: &ClientError) -> AccountsError {
    AccountsError::Unavailable {
        code: error.code().to_owned(),
        message: error.message(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use silicon_accounts_client::ProofVerification;
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, header_exists, method, path},
    };

    use super::{AccountsError, AccountsGateway, Introspection, test_support::TestIssuer};
    use crate::config::{AccountsSettings, ProofIssuers};

    fn settings(server: &MockServer) -> anyhow::Result<AccountsSettings> {
        let url = Url::parse(&server.uri())?;
        Ok(AccountsSettings {
            public_url: url.clone(),
            issuer: server.uri(),
            api_url: url,
            app_id: "briefcase".to_owned(),
            app_secret: secrecy::SecretString::from(
                "sa_app_briefcase_test_secret_0123456789".to_owned(),
            ),
            request_timeout: Duration::from_secs(2),
            account_cache_ttl: Duration::from_secs(300),
            proof_issuers: ProofIssuers::default(),
        })
    }

    fn code(result: Result<impl std::fmt::Debug, AccountsError>) -> String {
        match result {
            Err(AccountsError::InvalidToken { code, .. }) => code.to_owned(),
            other => format!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn tokens_verify_locally_and_every_rejection_is_named() -> anyhow::Result<()> {
        let server = MockServer::start().await;
        let issuer = TestIssuer::new(7, "k7", &server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issuer.jwks_json()))
            .expect(1)
            .mount(&server)
            .await;
        let gateway = AccountsGateway::new(&settings(&server)?)?;

        let claims = gateway
            .verify_access_token(&issuer.token("zQo", "carbon", "c:ada"))
            .await?;
        assert_eq!(claims.sub, "zQo");
        assert_eq!(claims.id.as_deref(), Some("c:ada"));

        let mut wrong_audience = issuer.claims("zQo", "carbon", "c:ada", 0, 600);
        wrong_audience["aud"] = json!("remind");
        assert_eq!(
            code(
                gateway
                    .verify_access_token(&issuer.sign(&wrong_audience))
                    .await
            ),
            "token_wrong_audience"
        );
        let mut wrong_issuer = issuer.claims("zQo", "carbon", "c:ada", 0, 600);
        wrong_issuer["iss"] = json!("https://accounts.example");
        assert_eq!(
            code(
                gateway
                    .verify_access_token(&issuer.sign(&wrong_issuer))
                    .await
            ),
            "token_wrong_issuer"
        );
        let expired = issuer.claims("zQo", "carbon", "c:ada", -7_200, 1_800);
        assert_eq!(
            code(gateway.verify_access_token(&issuer.sign(&expired)).await),
            "token_expired"
        );
        // No default 30-second grace period for expired or future credentials.
        let just_expired = issuer.claims("zQo", "carbon", "c:ada", -31, 30);
        assert_eq!(
            code(
                gateway
                    .verify_access_token(&issuer.sign(&just_expired))
                    .await
            ),
            "token_expired"
        );
        let future = issuer.claims("zQo", "carbon", "c:ada", 20, 600);
        assert_eq!(
            code(gateway.verify_access_token(&issuer.sign(&future)).await),
            "token_not_yet_valid"
        );
        let forged = TestIssuer::new(9, "k7", &server.uri()).token("zQo", "carbon", "c:ada");
        assert_eq!(
            code(gateway.verify_access_token(&forged).await),
            "token_bad_signature"
        );
        assert_eq!(
            code(gateway.verify_access_token("not.a.jwt").await),
            "token_malformed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_unknown_key_refetches_once_per_interval() -> anyhow::Result<()> {
        let server = MockServer::start().await;
        let old = TestIssuer::new(7, "k-old", &server.uri());
        let rotated = TestIssuer::new(8, "k-new", &server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rotated.jwks_json()))
            .expect(1)
            .mount(&server)
            .await;
        let gateway = AccountsGateway::new(&settings(&server)?)?.with_jwks(old.jwks());

        // Signed with the rotated key: one refetch finds it.
        let claims = gateway
            .verify_access_token(&rotated.token("zQo", "carbon", "c:ada"))
            .await?;
        assert_eq!(claims.sub, "zQo");
        // A key nobody publishes is refused without another fetch.
        let stranger = TestIssuer::new(3, "k-stranger", &server.uri());
        assert_eq!(
            code(
                gateway
                    .verify_access_token(&stranger.token("zQo", "carbon", "c:ada"))
                    .await
            ),
            "token_unknown_key"
        );
        Ok(())
    }

    #[tokio::test]
    async fn introspection_rechecks_class_audience_and_revocation_every_time() -> anyhow::Result<()>
    {
        let server = MockServer::start().await;
        let gateway = AccountsGateway::new(&settings(&server)?)?;
        for answer in [
            json!({"active": true, "sub": "zQo", "client_id": "briefcase", "token_type": "access_token"}),
            json!({"active": false}),
            json!({"active": true, "sub": "zQo", "client_id": "briefcase", "token_type": "refresh_token"}),
            json!({"active": true, "sub": "zQo", "client_id": "other", "token_type": "access_token"}),
            json!({"active": true, "sub": "zQo", "client_id": "briefcase"}),
            json!({"active": true, "sub": "zQo", "token_type": "access_token"}),
        ] {
            server.reset().await;
            Mock::given(method("POST"))
                .and(path("/v1/oauth/introspect"))
                .and(header_exists("authorization"))
                .and(body_string_contains("token=same-token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(answer.clone()))
                .expect(2)
                .mount(&server)
                .await;
            let expected = if answer["active"] == true
                && answer["client_id"] == "briefcase"
                && answer["token_type"] == "access_token"
            {
                Introspection::Active {
                    subject: Some("zQo".to_owned()),
                }
            } else {
                Introspection::Inactive
            };
            for _ in 0..2 {
                assert_eq!(gateway.introspect("same-token").await?, expected);
            }
        }
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/oauth/introspect"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        assert!(matches!(
            gateway.introspect("same-token").await,
            Err(AccountsError::Unavailable { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn proofs_lookups_and_app_credentials_map_to_precise_outcomes() -> anyhow::Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/proofs/verify"))
            .and(body_string_contains("sap_valid"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                super::test_support::valid_proof(
                    "waveform",
                    "briefcase",
                    ("zQo", "c:ada", "carbon"),
                    &["briefcase.files.read"],
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/proofs/verify"))
            .and(body_string_contains("sap_gone"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"valid": false, "expires_at": null})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts/by-id/c:nobody"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": {"code": "account_not_found", "message": "No account has the id c:nobody."}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/accounts/zQo"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": {"code": "invalid_app_credentials", "message": "The app secret is wrong."}
            })))
            .mount(&server)
            .await;
        let gateway = AccountsGateway::new(&settings(&server)?)?;

        let ProofVerification::Valid(proof) = gateway.verify_proof("sap_valid").await? else {
            anyhow::bail!("the stub proof must verify");
        };
        assert_eq!(proof.issuing_app.app_id, "waveform");
        assert_eq!(proof.user.map(|user| user.uuid).as_deref(), Some("zQo"));
        assert_eq!(
            gateway.verify_proof("sap_gone").await?,
            ProofVerification::Invalid
        );
        assert!(gateway.lookup_by_id("c:nobody").await?.is_none());
        assert!(matches!(
            gateway.lookup("zQo").await,
            Err(AccountsError::CredentialsRejected { code, .. }) if code == "invalid_app_credentials"
        ));
        Ok(())
    }
}
