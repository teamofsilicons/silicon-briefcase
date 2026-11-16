//! Organization membership and role-checked management through Briefcase.
use crate::{Client, IdempotencyKey, Result};
use reqwest::Method;
use serde_json::Value;

impl Client {
    /// Lists this account's organizations and current sharing targets.
    /// Linked personal or work accounts do not confer authority.
    /// # Errors
    /// Returns authentication or service availability errors.
    pub async fn organizations(&self) -> Result<Value> {
        self.receive_json(
            self.request(Method::GET, self.api_url(&["organizations"])?)
                .timeout(self.request_timeout()),
        )
        .await
    }

    /// Reads visible members, teams and management capabilities.
    /// # Errors
    /// Returns an error if the account cannot access this organization.
    pub async fn organization(&self, organization: &str) -> Result<Value> {
        self.receive_json(
            self.request(Method::GET, self.api_url(&["organizations", organization])?)
                .timeout(self.request_timeout()),
        )
        .await
    }

    /// Applies an Organizations action using current role checks and audit.
    /// Reuse the key and exact body when retrying an uncertain outcome.
    /// # Errors
    /// Returns authorization, validation, idempotency or availability errors.
    pub async fn organization_action(
        &self,
        organization: &str,
        body: &Value,
        key: &IdempotencyKey,
    ) -> Result<Value> {
        self.receive_json(
            self.request(
                Method::POST,
                self.api_url(&["organizations", organization, "actions"])?,
            )
            .header("idempotency-key", key.as_str())
            .json(body)
            .timeout(self.request_timeout()),
        )
        .await
    }
}
