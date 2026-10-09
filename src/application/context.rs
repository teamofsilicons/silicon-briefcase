//! Trusted execution context shared by application use cases.

use crate::domain::actor::RequestAuthContext;

/// Verified authorization facts plus request correlation for one use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    authorization: RequestAuthContext,
    request_id: String,
    directory_members: Vec<RequestAuthContext>,
}

impl ExecutionContext {
    /// Constructs a context after the caller's credential was verified.
    #[must_use]
    pub fn new(authorization: RequestAuthContext, request_id: impl Into<String>) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
            directory_members: Vec::new(),
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

    /// Attaches directory projections of share recipients, never caller authentication.
    pub(crate) fn with_directory_members(mut self, members: Vec<RequestAuthContext>) -> Self {
        self.directory_members = members;
        self
    }

    pub(crate) fn directory_members(&self) -> &[RequestAuthContext] {
        &self.directory_members
    }
}
