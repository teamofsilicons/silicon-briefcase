//! Request-local correlation and IAM-verified authorization context.

use std::{future::Future, sync::Arc};

use crate::domain::actor::RequestAuthContext;

tokio::task_local! {
    static CURRENT_CONTEXT: RequestContext;
}

/// Correlation and optional authenticated facts scoped to one request task.
#[derive(Clone, Debug)]
pub struct RequestContext {
    request_id: Arc<str>,
    authorization: Option<Arc<RequestAuthContext>>,
}

impl RequestContext {
    /// Creates a request scope before authentication has completed.
    #[must_use]
    pub fn unauthenticated(request_id: impl Into<Arc<str>>) -> Self {
        Self {
            request_id: request_id.into(),
            authorization: None,
        }
    }

    /// Creates a request scope with facts established by online IAM verification.
    #[must_use]
    pub fn authenticated(
        request_id: impl Into<Arc<str>>,
        authorization: RequestAuthContext,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            authorization: Some(Arc::new(authorization)),
        }
    }

    /// Returns the request correlation identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    /// Returns the trusted IAM authorization facts, when authentication succeeded.
    #[must_use]
    pub fn authorization(&self) -> Option<&RequestAuthContext> {
        self.authorization.as_deref()
    }
}

/// Runs a future with only a validated correlation identifier.
pub async fn scope<T>(request_id: String, future: impl Future<Output = T>) -> T {
    scope_context(RequestContext::unauthenticated(request_id), future).await
}

/// Runs a future with correlation and IAM-verified authorization facts.
pub async fn scope_authenticated<T>(
    request_id: String,
    authorization: RequestAuthContext,
    future: impl Future<Output = T>,
) -> T {
    scope_context(
        RequestContext::authenticated(request_id, authorization),
        future,
    )
    .await
}

/// Runs a future inside an explicitly constructed request scope.
pub async fn scope_context<T>(context: RequestContext, future: impl Future<Output = T>) -> T {
    CURRENT_CONTEXT.scope(context, future).await
}

/// Returns a clone of the active request context, when called inside a scope.
#[must_use]
pub fn current() -> Option<RequestContext> {
    CURRENT_CONTEXT.try_with(Clone::clone).ok()
}

/// Returns the active request ID, when called inside a request scope.
#[must_use]
pub fn current_request_id() -> Option<String> {
    CURRENT_CONTEXT
        .try_with(|context| context.request_id().to_owned())
        .ok()
}

/// Returns the IAM-verified authorization facts for the active scope.
#[must_use]
pub fn current_authorization() -> Option<Arc<RequestAuthContext>> {
    CURRENT_CONTEXT
        .try_with(|context| context.authorization.clone())
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{current_authorization, current_request_id, scope};

    #[tokio::test]
    async fn request_id_is_available_only_inside_its_scope() {
        assert_eq!(current_request_id(), None);

        let observed = scope("request-1".to_owned(), async { current_request_id() }).await;

        assert_eq!(observed.as_deref(), Some("request-1"));
        assert_eq!(current_request_id(), None);
        assert!(current_authorization().is_none());
    }

    #[tokio::test]
    async fn nested_scopes_restore_the_outer_context() {
        let observed = scope("outer".to_owned(), async {
            let inner = scope("inner".to_owned(), async { current_request_id() }).await;
            (inner, current_request_id())
        })
        .await;

        assert_eq!(observed.0.as_deref(), Some("inner"));
        assert_eq!(observed.1.as_deref(), Some("outer"));
    }
}
