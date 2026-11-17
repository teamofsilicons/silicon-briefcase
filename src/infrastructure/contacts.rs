//! Scoped, bounded access to the account's Silicon Contacts address book.
use crate::error::AppError;
use reqwest::{Client, Method};
use secrecy::ExposeSecret as _;
use serde_json::Value;
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub(crate) struct ContactsGateway {
    base: Url,
    http: Client,
}

pub(crate) fn unavailable() -> AppError {
    AppError::Explained {
        status: http::StatusCode::SERVICE_UNAVAILABLE,
        code: "contacts_unavailable".into(),
        message:
            "Silicon Contacts could not be reached. Your contacts have not changed. Try again."
                .into(),
    }
}

impl ContactsGateway {
    pub(crate) fn new(base: &str) -> Result<Self, AppError> {
        let base = Url::parse(base).map_err(|_| unavailable())?;
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
            || !(base.scheme() == "https"
                || (base.scheme() == "http"
                    && matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return Err(unavailable());
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self { base, http })
    }

    pub(crate) async fn bytes(
        &self,
        proof: &str,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Vec<u8>, AppError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| unavailable())?
            .extend(["v1"])
            .extend(segments.iter().copied());
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        let mut request = self
            .http
            .request(method, url)
            .header("Authorization", format!("Proof {proof}"));
        if let Some(body) = body {
            request = request.json(body);
        }
        let mut response = request.send().await.map_err(|_| unavailable())?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > 5 * 1024 * 1024 {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("Contacts could not complete this request.");
            return Err(match status.as_u16() {
                404 => AppError::NotFound,
                400 | 409 | 422 => AppError::unprocessable("contact_invalid", message.to_owned()),
                401 | 403 => AppError::refused(
                    "contacts_access_denied",
                    "Contacts access was refused. Reconnect Contacts or check your account’s app permissions.",
                ),
                _ => unavailable(),
            });
        }
        Ok(bytes)
    }

    pub(crate) async fn json(
        &self,
        proof: &str,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value, AppError> {
        let bytes = self.bytes(proof, method, segments, query, body).await?;
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|_| unavailable())
    }

    pub(crate) async fn is_saved_sender(
        &self,
        accounts: &crate::infrastructure::accounts::AccountsGateway,
        recipient: &str,
        sender: &str,
    ) -> Result<bool, AppError> {
        let proof = accounts.share_check_proof(recipient, sender).await?;
        let decision = self
            .json(
                proof.expose_secret(),
                Method::POST,
                &["integrations", "briefcase", "share-check"],
                &[],
                Some(&serde_json::json!({ "recipient": recipient, "sender": sender })),
            )
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                dependency: "silicon_contacts",
            })?;
        decision["saved"]
            .as_bool()
            .ok_or(AppError::DependencyUnavailable {
                dependency: "silicon_contacts",
            })
    }

    pub(crate) async fn all(&self, proof: &str) -> Result<Vec<Value>, AppError> {
        let mut items = Vec::new();
        loop {
            let page = self
                .json(
                    proof,
                    Method::GET,
                    &["contacts"],
                    &[("limit", "200".into()), ("offset", items.len().to_string())],
                    None,
                )
                .await?;
            let batch = page["items"].as_array().ok_or_else(unavailable)?;
            let total = page["total"].as_u64().ok_or_else(unavailable)?;
            if batch.is_empty() && (items.len() as u64) < total {
                return Err(unavailable());
            }
            items.extend(batch.iter().cloned());
            if items.len() as u64 >= total {
                return Ok(items);
            }
            // Refuse an incomplete address book instead of silently misranking/filtering it.
            if items.len() >= 10_000 {
                return Err(AppError::unprocessable(
                    "contacts_too_many",
                    "This address book is too large to load in Briefcase. Open Silicon Contacts to manage it.",
                ));
            }
        }
    }
}
