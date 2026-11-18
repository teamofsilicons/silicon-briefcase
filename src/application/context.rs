//! Trusted execution context shared by application use cases.

use crate::domain::{
    actor::{ActorRef, RequestAuthContext},
    organizations::OrganizationContext,
};

/// Verified authorization facts plus request correlation for one use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    authorization: RequestAuthContext,
    request_id: String,
    share_recipients: Vec<ActorRef>,
    review_recipients: Vec<ActorRef>,
    organizations: OrganizationContext,
}

impl ExecutionContext {
    /// Constructs a context after the caller's credential was verified.
    #[must_use]
    pub fn new(authorization: RequestAuthContext, request_id: impl Into<String>) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
            share_recipients: Vec::new(),
            review_recipients: Vec::new(),
            organizations: OrganizationContext::default(),
        }
    }

    /// Returns the verified authorization facts.
    #[must_use]
    pub const fn authorization(&self) -> &RequestAuthContext {
        &self.authorization
    }

    /// Returns the validated request correlation identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Replaces the authorization facts, keeping the request correlation, for
    /// a request moved into the drive it acts on.
    #[must_use]
    pub fn with_authorization(mut self, authorization: RequestAuthContext) -> Self {
        self.authorization = authorization;
        self
    }

    /// Attaches accounts a share names, resolved at Silicon Accounts. They
    /// become participants of the drive so grants can name them; they never
    /// act in this request.
    #[must_use]
    pub fn with_share_recipients(mut self, recipients: Vec<ActorRef>) -> Self {
        self.share_recipients = recipients;
        self
    }

    /// Adds one directory-resolved account and the delivery decision for this request.
    pub(crate) fn add_share_recipient(&mut self, recipient: &ActorRef, needs_review: bool) {
        self.share_recipients.push(recipient.clone());
        if needs_review {
            self.review_recipients.push(recipient.clone());
        }
    }

    pub(crate) fn review_recipients(&self) -> &[ActorRef] {
        &self.review_recipients
    }

    /// Returns the accounts a share in this request names.
    #[must_use]
    pub fn share_recipients(&self) -> &[ActorRef] {
        &self.share_recipients
    }

    /// Attaches the current Organizations response bound to this account.
    #[must_use]
    pub fn with_organizations(mut self, organizations: OrganizationContext) -> Self {
        self.organizations = organizations;
        self
    }

    /// Live organization and team authority for this request only.
    #[must_use]
    pub const fn organizations(&self) -> &OrganizationContext {
        &self.organizations
    }
}
