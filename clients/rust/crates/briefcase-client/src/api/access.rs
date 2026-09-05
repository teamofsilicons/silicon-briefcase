//! Grants, access requests, and the notification inbox.

use reqwest::Method;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    client::{Client, IdempotencyKey, json_body},
    error::Result,
    models::{
        AccessRequest, AccessRight, NotificationInbox, PermissionGrant, PermissionGrantPage,
        PermissionInspection,
    },
    requests::{AccessDecision, NewAccessRequest, NewGrant, PermissionQuery},
};

#[derive(Serialize)]
struct WireDecision<'a> {
    decision: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<&'a [AccessRight]>,
}

#[derive(Serialize)]
struct WirePathAccessRequest<'a> {
    path: &'a str,
    access: &'a [AccessRight],
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

impl Client {
    /// Lists the explicit grants on an entry.
    ///
    /// These are the grants somebody made, not every way the caller might have
    /// reached the entry: Public visibility, a tag, ownership, and
    /// administrative authority convey access without a grant.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is not visible to the caller.
    pub async fn permissions(&self, entry_id: Uuid) -> Result<Vec<PermissionGrant>> {
        let url = self.api_url(&["entries", &entry_id.to_string(), "permissions"])?;
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        let page: PermissionGrantPage = self.receive_json(request).await?;
        Ok(page.items)
    }

    /// Grants a member access to an entry.
    ///
    /// Granting a member who already holds a grant amends it: the rights and
    /// inheritance become exactly what this call names, so widening access
    /// never has to pass through a revocation.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller cannot manage permissions there, or
    /// the principal is not a current member of the organization.
    pub async fn grant(&self, entry_id: Uuid, grant: &NewGrant) -> Result<PermissionGrant> {
        let url = self.api_url(&["entries", &entry_id.to_string(), "permissions"])?;
        let body = json_body(grant)?;
        let request = self
            .request(Method::POST, url)
            .header("content-type", "application/json")
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Revokes one explicit grant.
    ///
    /// Access the member has by another route — a second grant, a tag, Public
    /// visibility, ownership, administration — is untouched.
    ///
    /// # Errors
    ///
    /// Returns a not-found error when the grant is already revoked or never
    /// existed.
    pub async fn revoke(&self, entry_id: Uuid, grant_id: Uuid) -> Result<()> {
        let url = self.api_url(&[
            "entries",
            &entry_id.to_string(),
            "permissions",
            &grant_id.to_string(),
        ])?;
        let request = self
            .request(Method::DELETE, url)
            .timeout(self.request_timeout());
        self.receive_empty(request).await
    }

    /// Reports what the caller may do on up to a hundred named targets.
    ///
    /// A target that does not exist and one the caller cannot read are both
    /// reported as unresolved, so the answer cannot be used to probe.
    ///
    /// # Errors
    ///
    /// Returns an error when no target was named, or more than a hundred were.
    pub async fn effective_access(&self, query: &PermissionQuery) -> Result<PermissionInspection> {
        let url = self.api_url(&["permissions", "effective"])?;
        let body = json_body(query)?;
        let request = self
            .request(Method::POST, url)
            .header("content-type", "application/json")
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Asks the owner and organization administrators for access.
    ///
    /// This works on an entry the caller cannot read: it is what a member does
    /// after opening a permanent URL that answered as missing.
    ///
    /// # Errors
    ///
    /// Returns a not-found error when the entry does not exist at all.
    pub async fn request_access(
        &self,
        entry_id: Uuid,
        request: &NewAccessRequest,
    ) -> Result<AccessRequest> {
        let url = self.api_url(&["entries", &entry_id.to_string(), "access-requests"])?;
        let body = json_body(request)?;
        let http_request = self
            .request(Method::POST, url)
            .header("content-type", "application/json")
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(http_request).await
    }

    /// Asks for access using the organization-relative path from a permanent URL.
    ///
    /// Unlike ordinary path resolution, this operation works when the target is
    /// hidden from the caller. It returns only the access-request record and
    /// never exposes the entry's name, owner, or other metadata.
    ///
    /// # Errors
    ///
    /// Returns the same opaque not-found response for a missing path and for a
    /// path outside the configured organization.
    pub async fn request_access_by_path(
        &self,
        path: &str,
        request: &NewAccessRequest,
    ) -> Result<AccessRequest> {
        self.request_access_by_path_with_key(path, request, &IdempotencyKey::random())
            .await
    }

    /// Asks for access by path using a caller-owned retry identity.
    ///
    /// Persist the key with the exact path, rights, and reason before sending,
    /// then reuse that complete request after an uncertain outcome.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Client::request_access_by_path`].
    pub async fn request_access_by_path_with_key(
        &self,
        path: &str,
        request: &NewAccessRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<AccessRequest> {
        let url = self.api_url(&["access-requests"])?;
        let body = json_body(&WirePathAccessRequest {
            path,
            access: &request.access,
            reason: request.reason.as_deref(),
        })?;
        let http_request = self
            .request(Method::POST, url)
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key.as_str())
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(http_request).await
    }

    /// Approves or denies an access request.
    ///
    /// Approval creates the grant and notifies the requester; denial creates
    /// nothing. Either way the request is settled and cannot be decided twice.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller is not the owner or an administrator,
    /// or the request has already been decided.
    pub async fn decide_access_request(
        &self,
        request_id: Uuid,
        decision: &AccessDecision,
    ) -> Result<AccessRequest> {
        let url = self.api_url(&["access-requests", &request_id.to_string(), "decision"])?;
        let body = match decision {
            AccessDecision::Approve(access) => json_body(&WireDecision {
                decision: "approve",
                access: Some(access),
            })?,
            AccessDecision::Deny => json_body(&WireDecision {
                decision: "deny",
                access: None,
            })?,
        };
        let request = self
            .request(Method::POST, url)
            .header("content-type", "application/json")
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Reads the caller's twenty newest notifications and unread count.
    ///
    /// # Errors
    ///
    /// Returns an error when the deployment cannot be reached.
    pub async fn notifications(&self) -> Result<NotificationInbox> {
        let url = self.api_url(&["notifications"])?;
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Marks the whole inbox read and returns it afterwards.
    ///
    /// # Errors
    ///
    /// Returns an error when the deployment cannot be reached.
    pub async fn mark_notifications_read(&self) -> Result<NotificationInbox> {
        let url = self.api_url(&["notifications", "read"])?;
        let request = self
            .request(Method::POST, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }
}
