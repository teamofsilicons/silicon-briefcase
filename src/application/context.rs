//! Trusted execution context shared by application use cases.

use crate::domain::actor::{ActorRef, RequestAuthContext};

/// Verified authorization facts plus request correlation for one use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    authorization: RequestAuthContext,
    request_id: String,
    share_recipients: Vec<ActorRef>,
}

impl ExecutionContext {
    /// Constructs a context after the caller's credential was verified.
    #[must_use]
    pub fn new(authorization: RequestAuthContext, request_id: impl Into<String>) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
            share_recipients: Vec::new(),
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

    /// Returns the accounts a share in this request names.
    #[must_use]
    pub fn share_recipients(&self) -> &[ActorRef] {
        &self.share_recipients
    }
}
