//! Address-book consent, saved contacts and invitation suggestions.
use crate::{
    Client, Result,
    api::ContentStream,
    models::{Account, SharedPage},
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

/// Filters applied to shared entries before pagination.
#[derive(Clone, Debug, Default)]
pub struct SharedFilters<'a> {
    /// Next page cursor.
    pub cursor: Option<&'a str>,
    /// Page size, at most 100.
    pub limit: Option<u16>,
    /// Actual sharer's public ID or UUID.
    pub from: Option<&'a str>,
    /// `saved` or `unsaved`; requires Contacts connection.
    pub contacts: Option<&'a str>,
    /// Minimum saved-contact trust, 1–255.
    pub min_trust: Option<u8>,
    /// Case-insensitive filename/path match within shared files.
    pub query: Option<&'a str>,
}

/// A recipient with directional invitation counts and private saved details.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecipientProfile {
    /// Resolved account, with saved name/photo overrides.
    #[serde(flatten)]
    pub account: Account,
    /// Permitted account photo to try if a saved contact photo cannot load.
    pub fallback_pfp_url: Option<String>,
    /// Invitations the current account sent to this person.
    #[serde(default)]
    pub invited: u64,
    /// Invitations this person sent to the current account.
    #[serde(default)]
    pub received: u64,
    /// Most recent invitation in either direction, RFC 3339.
    pub last_invited_at: Option<String>,
    /// Saved name, trust and optional photo UUID.
    pub contact: Option<Value>,
}
/// Complete candidate index for local fuzzy filtering and weighted ranking.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecipientIndex {
    /// Candidates, deduplicated by immutable account UUID.
    pub items: Vec<RecipientProfile>,
    /// `connected`, `disconnected`, or `unavailable`.
    pub contacts_status: String,
}

impl Client {
    /// Reads private candidates. Rank by `2*invited+received`, recency, then contact trust.
    /// # Errors
    /// Returns authentication or storage errors; Contacts outages are reported in the result.
    pub async fn recipients(&self) -> Result<RecipientIndex> {
        self.receive_json(
            self.request(Method::GET, self.api_url(&["recipients"])?)
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Resolves an exact ID/UUID with the caller's saved name and photo.
    /// # Errors
    /// Returns unknown-account, authorization or dependency errors.
    pub async fn resolve_recipient(&self, id: &str) -> Result<RecipientProfile> {
        let mut url = self.api_url(&["recipients", "resolve"])?;
        url.query_pairs_mut().append_pair("id", id);
        self.receive_json(
            self.request(Method::GET, url)
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Reads the account's complete Silicon Contacts address book.
    /// # Errors
    /// Returns authentication or dependency errors.
    pub async fn contacts(&self) -> Result<Value> {
        self.receive_json(
            self.request(Method::GET, self.api_url(&["contacts"])?)
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Connects after explicit account consent, or disconnects without deleting contacts.
    /// # Errors
    /// Returns authentication or Contacts verification errors.
    pub async fn connect_contacts(&self, enabled: bool) -> Result<Value> {
        self.receive_json(
            self.request(Method::PUT, self.api_url(&["contacts", "connection"])?)
                .json(&json!({"enabled":enabled}))
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Saves a name and trust (1–255), preserving other existing contact fields.
    /// # Errors
    /// Returns invalid-input, missing-consent, or dependency errors.
    pub async fn save_contact(&self, id: &str, name: &str, trust: u8) -> Result<Value> {
        self.receive_json(
            self.request(Method::POST, self.api_url(&["contacts"])?)
                .json(&json!({"id":id,"name":name,"trust":trust}))
                .timeout(self.request_timeout()),
        )
        .await
    }
    /// Reads a private contact photo with the caller's address-book authorization.
    /// # Errors
    /// Returns missing-consent, not-found or dependency errors.
    pub async fn contact_photo(&self, id: Uuid) -> Result<ContentStream> {
        self.open_content(
            self.api_url(&["contacts", "photos", &id.to_string()])?,
            None,
        )
        .await
    }
    /// Lists shared files with all filters applied together before pagination.
    /// # Errors
    /// Returns invalid filters, missing Contacts consent or dependency errors.
    pub async fn shared_with_filters(&self, filters: &SharedFilters<'_>) -> Result<SharedPage> {
        let mut url = self.api_url(&["shared"])?;
        {
            let mut query = url.query_pairs_mut();
            for (key, value) in [
                ("cursor", filters.cursor),
                ("from", filters.from),
                ("contacts", filters.contacts),
                ("q", filters.query),
            ] {
                if let Some(value) = value {
                    query.append_pair(key, value);
                }
            }
            if let Some(limit) = filters.limit {
                query.append_pair("limit", &limit.to_string());
            }
            if let Some(trust) = filters.min_trust {
                query.append_pair("min_trust", &trust.to_string());
            }
        }
        self.receive_json(
            self.request(Method::GET, url)
                .timeout(self.request_timeout()),
        )
        .await
    }
}
