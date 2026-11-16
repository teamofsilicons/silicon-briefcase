//! Live Organizations authority for Briefcase work accounts and dynamic shares.
use crate::error::AppError;
use reqwest::{Client, Method};
use serde_json::Value;
use std::time::Duration;
use url::Url;

pub use crate::domain::organizations::{OrganizationContext, ShareTarget};

/// Bounded transport to Organizations' shared app-session API for Briefcase.
#[derive(Clone)]
pub struct OrganizationsGateway {
    base: Url,
    http: Client,
}

fn unavailable() -> AppError {
    AppError::Explained { status: http::StatusCode::SERVICE_UNAVAILABLE, code: "organizations_unavailable".into(), message: "Organization access could not be verified. Try again when Silicon Organizations is available.".into() }
}

impl OrganizationsGateway {
    /// Builds a gateway. Only HTTPS or local development HTTP is accepted.
    /// # Errors
    /// Returns an error for unsafe URLs or a failed HTTP client initialization.
    pub fn new(base: &str) -> Result<Self, AppError> {
        let base = Url::parse(base).map_err(|_| unavailable())?;
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !(base.scheme() == "https"
                || (base.scheme() == "http"
                    && matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            || base.path() != "/"
        {
            return Err(unavailable());
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self { base, http })
    }

    /// Makes a credential-bound request; neither upstream tokens nor headers
    /// are returned to browser clients.
    /// # Errors
    /// Returns upstream authorization errors or an availability error for invalid responses.
    pub async fn request(
        &self,
        token: &str,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
        key: Option<&str>,
    ) -> Result<Value, AppError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| unavailable())?
            .extend(["v1", "integrations", "briefcase"])
            .extend(segments.iter().copied());
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        let mut response = request.send().await.map_err(|_| unavailable())?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if !status.is_success() {
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("Organizations could not complete this request.")
                .to_owned();
            return Err(match status.as_u16() {
                401 => AppError::unauthorized("organizations_session_ended", message),
                403 => AppError::refused("organizations_forbidden", message),
                404 => AppError::NotFound,
                409 => AppError::Explained {
                    status: http::StatusCode::CONFLICT,
                    code: "organization_changed".into(),
                    message: message.into(),
                },
                400 | 422 => AppError::unprocessable("organization_action_invalid", message),
                _ => unavailable(),
            });
        }
        Ok(value)
    }

    /// Cross-binds membership to the account already verified by Briefcase.
    /// # Errors
    /// Returns an error when current authority is unavailable or names another account.
    pub async fn context(
        &self,
        token: &str,
        account: &str,
    ) -> Result<OrganizationContext, AppError> {
        let value = self
            .request(token, Method::GET, &["me"], None, None)
            .await?;
        let context: OrganizationContext =
            serde_json::from_value(value).map_err(|_| unavailable())?;
        validate_context(&context, account)?;
        Ok(context)
    }
}

fn validate_context(context: &OrganizationContext, account: &str) -> Result<(), AppError> {
    if context.account_uuid != account
        || context.groups.len() > 4096
        || context.share_targets.len() > 4096
        || context.groups.iter().any(|group| {
            group.len() > 200 || !group.starts_with("organization:") && !group.starts_with("team:")
        })
        || context.share_targets.iter().any(|target| {
            !matches!(target.kind.as_str(), "organization" | "team")
                || target.id.is_empty()
                || target.id.len() > 180
        })
    {
        return Err(unavailable());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cross_account_membership_and_unsafe_transport() {
        let context = OrganizationContext {
            account_uuid: "alice".into(),
            ..Default::default()
        };
        assert!(validate_context(&context, "alice").is_ok());
        assert!(validate_context(&context, "bob").is_err());
        for url in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://example.com/other",
            "https://example.com/?token=x",
        ] {
            assert!(OrganizationsGateway::new(url).is_err());
        }
        assert!(OrganizationsGateway::new("http://127.0.0.1:8660").is_ok());
    }
}
