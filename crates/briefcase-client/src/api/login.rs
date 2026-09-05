//! IAM short-lived-token login and rotating session refresh.

use reqwest::Method;
use serde::Serialize;

use crate::{
    client::{Client, IdempotencyKey, json_body},
    error::{Error, Result},
    models::SessionTokens,
};

#[derive(Serialize)]
struct SltExchange<'a> {
    slt: &'a str,
}

#[derive(Serialize)]
struct RefreshExchange<'a> {
    refresh_token: &'a str,
}

impl Client {
    /// Exchanges a two-minute, single-use IAM SLT for a Briefcase session.
    ///
    /// The Briefcase Application secret remains on the backend. This client
    /// never accepts, stores, or transmits it.
    ///
    /// # Errors
    ///
    /// Returns an unauthenticated error when the SLT is unknown, expired,
    /// already spent, or was minted for another canonical Application ID.
    /// Returns [`Error::Protocol`] if Briefcase returns a session that is not
    /// bound to the organization configured on this client.
    pub async fn login_with_slt(&self, slt: &str) -> Result<SessionTokens> {
        self.login_with_slt_with_key(slt, &IdempotencyKey::random())
            .await
    }

    /// Exchanges an SLT using a caller-owned retry identity.
    ///
    /// Reuse the same key with the same SLT after an uncertain transport
    /// outcome. Supplying a new key can turn a successful-but-lost response
    /// into a terminal spent-token error.
    ///
    /// # Errors
    ///
    /// Returns an error for a short idempotency key, a refused exchange, or a
    /// session that is not bound to the organization configured on this
    /// client.
    pub async fn login_with_slt_with_key(
        &self,
        slt: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Result<SessionTokens> {
        self.session_exchange("slt", json_body(&SltExchange { slt })?, idempotency_key)
            .await
    }

    /// Rotates a refresh token and returns the next access/refresh pair.
    ///
    /// Persist the returned refresh token before retrying later work: successful
    /// refresh invalidates the token supplied here.
    ///
    /// # Errors
    ///
    /// Returns an unauthenticated error for an expired, spent, or cross-plane
    /// refresh token. Returns [`Error::Protocol`] if Briefcase returns a
    /// session that is not bound to the organization configured on this
    /// client.
    pub async fn refresh_session(&self, refresh_token: &str) -> Result<SessionTokens> {
        self.refresh_session_with_key(refresh_token, &IdempotencyKey::random())
            .await
    }

    /// Rotates a session using a caller-owned retry identity.
    ///
    /// Persist `idempotency_key` beside the supplied refresh token before the
    /// request. Reuse that exact pair after an uncertain outcome, then replace
    /// both tokens atomically on success.
    ///
    /// # Errors
    ///
    /// Returns an error for a short idempotency key, a refused refresh, or a
    /// session that is not bound to the organization configured on this
    /// client.
    pub async fn refresh_session_with_key(
        &self,
        refresh_token: &str,
        idempotency_key: &IdempotencyKey,
    ) -> Result<SessionTokens> {
        self.session_exchange(
            "refresh",
            json_body(&RefreshExchange { refresh_token })?,
            idempotency_key,
        )
        .await
    }

    async fn session_exchange(
        &self,
        operation: &str,
        body: Vec<u8>,
        idempotency_key: &IdempotencyKey,
    ) -> Result<SessionTokens> {
        if !(16..=255).contains(&idempotency_key.as_str().len())
            || !idempotency_key
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
        {
            return Err(Error::Configuration(
                "an auth idempotency key must be 16 to 255 visible ASCII bytes".to_owned(),
            ));
        }
        let url = self.api_url(&["auth", operation])?;
        let request = self
            .anonymous_request(Method::POST, url)
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key.as_str())
            .body(body)
            .timeout(self.request_timeout());
        let tokens: SessionTokens = self.receive_json_without_maintenance(request).await?;
        require_session_organization(tokens.org_id.as_deref(), self.organization())?;
        Ok(tokens)
    }
}

fn require_session_organization(actual: Option<&str>, expected: &str) -> Result<()> {
    match actual {
        Some(actual) if actual == expected => Ok(()),
        Some(actual) => Err(Error::Protocol(format!(
            "Briefcase returned a session for organization {actual}, but this client is configured for {expected}"
        ))),
        None => Err(Error::Protocol(format!(
            "Briefcase returned an organization-unbound session, but this client is configured for {expected}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::require_session_organization;

    #[test]
    fn session_organization_must_exactly_match_the_client() {
        assert!(require_session_organization(Some("tos"), "tos").is_ok());

        let missing = require_session_organization(None, "tos").unwrap_err();
        assert!(missing.to_string().contains("organization-unbound"));

        let mismatched = require_session_organization(Some("other"), "tos").unwrap_err();
        assert!(mismatched.to_string().contains("organization other"));
        assert!(mismatched.to_string().contains("configured for tos"));
    }
}
