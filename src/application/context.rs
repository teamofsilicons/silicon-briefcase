//! Trusted execution context shared by application use cases.

use crate::domain::actor::RequestAuthContext;

/// IAM authorization facts plus request correlation for one use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    authorization: RequestAuthContext,
    request_id: String,
}

impl ExecutionContext {
    /// Constructs a context after online IAM verification.
    #[must_use]
    pub fn new(authorization: RequestAuthContext, request_id: impl Into<String>) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
        }
    }

    /// Returns IAM-verified authorization facts.
    #[must_use]
    pub const fn authorization(&self) -> &RequestAuthContext {
        &self.authorization
    }

    /// Returns the validated request correlation identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}
