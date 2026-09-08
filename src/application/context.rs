//! Trusted execution context shared by application use cases.

use crate::domain::actor::RequestAuthContext;
use uuid::Uuid;

/// Public selector for an isolated Briefcase testing environment.
///
/// Root keys and IAM credentials deliberately never enter use-case contexts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestingEnvironmentContext {
    id: Uuid,
    control_version: i64,
}

impl TestingEnvironmentContext {
    /// Constructs a selector after its root key and control-plane generation
    /// were authenticated together.
    #[must_use]
    pub const fn new(id: Uuid, control_version: i64) -> Self {
        Self {
            id,
            control_version,
        }
    }

    /// Returns the environment's public UUID.
    #[must_use]
    pub const fn id(self) -> Uuid {
        self.id
    }

    /// Returns the control-plane generation authenticated with the root key.
    ///
    /// Data-plane transactions revalidate this value after acquiring their
    /// shared clean fence, so a request that waited behind a clean cannot
    /// publish into the newly emptied environment.
    #[must_use]
    pub const fn control_version(self) -> i64 {
        self.control_version
    }
}

/// IAM authorization facts plus request correlation for one use case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionContext {
    authorization: RequestAuthContext,
    request_id: String,
    testing_environment: Option<TestingEnvironmentContext>,
    directory_members: Vec<RequestAuthContext>,
}

impl ExecutionContext {
    /// Constructs a context after online IAM verification.
    #[must_use]
    pub fn new(authorization: RequestAuthContext, request_id: impl Into<String>) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
            testing_environment: None,
            directory_members: Vec::new(),
        }
    }

    /// Constructs a context for an authenticated test-plane request.
    #[must_use]
    pub fn in_testing_environment(
        authorization: RequestAuthContext,
        request_id: impl Into<String>,
        testing_environment: TestingEnvironmentContext,
    ) -> Self {
        Self {
            authorization,
            request_id: request_id.into(),
            testing_environment: Some(testing_environment),
            directory_members: Vec::new(),
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

    /// Returns the selected sandbox, or `None` for production.
    #[must_use]
    pub const fn testing_environment(&self) -> Option<TestingEnvironmentContext> {
        self.testing_environment
    }

    /// Attaches online directory projections, never caller authentication.
    pub(crate) fn with_directory_members(mut self, members: Vec<RequestAuthContext>) -> Self {
        self.directory_members = members;
        self
    }

    pub(crate) fn directory_members(&self) -> &[RequestAuthContext] {
        &self.directory_members
    }
}
