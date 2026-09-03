//! Stable application errors and the public `OpenAPI` error envelope.

use std::borrow::Cow;

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

use crate::domain::quota::UploadLimit;

/// Error returned by a Briefcase use case or transport boundary.
#[derive(Debug, Error)]
pub enum AppError {
    /// A syntactically malformed request cannot be interpreted.
    #[error("the request is malformed")]
    BadRequest {
        /// Stable machine-readable reason.
        code: Cow<'static, str>,
    },
    /// Parsed input violates a domain validation rule.
    #[error("request validation failed")]
    Validation {
        /// Stable machine-readable validation reason.
        code: Cow<'static, str>,
    },
    /// Credential is absent, invalid, expired, or revoked.
    #[error("authentication is required")]
    Unauthenticated,
    /// The authenticated actor lacks authority for the action.
    #[error("the actor is not authorized for this action")]
    Forbidden,
    /// The resource does not exist or must remain undisclosed.
    #[error("resource was not found")]
    NotFound,
    /// The operation conflicts with current durable state.
    #[error("request conflicts with current state")]
    Conflict {
        /// Stable machine-readable conflict reason.
        code: Cow<'static, str>,
    },
    /// The body exceeds the route-specific byte limit.
    #[error("request body is too large")]
    PayloadTooLarge,
    /// The requested byte range lies outside the file.
    #[error("requested range is not satisfiable")]
    RangeNotSatisfiable {
        /// Complete size of the addressed content.
        total_size: u64,
    },
    /// The organization reached its daily upload allowance or storage ceiling.
    #[error("organization limit exhausted: {limit}")]
    UploadLimitExhausted {
        /// Which limit stopped the write.
        limit: UploadLimit,
        /// Seconds until the capacity returns, when waiting alone restores it.
        retry_after_seconds: Option<u64>,
    },
    /// A caller exceeded an abuse or capacity limit.
    #[error("rate limit exceeded")]
    RateLimited {
        /// Seconds after which the caller may retry.
        retry_after_seconds: u64,
    },
    /// Request processing exceeded its deadline.
    #[error("request processing deadline exceeded")]
    Timeout,
    /// The caller supports no API version this build serves.
    #[error("no mutually supported API version")]
    UnsupportedApiVersion,
    /// The route exists but does not accept this method.
    #[error("method is not allowed for this route")]
    MethodNotAllowed,
    /// A required dependency is unavailable or returned an unsafe response.
    #[error("required dependency is unavailable: {dependency}")]
    DependencyUnavailable {
        /// Static dependency label safe for logs.
        dependency: &'static str,
    },
    /// PostgreSQL may have committed an idempotent publication before the
    /// connection failed, so compensating external state would be unsafe.
    #[error("database commit outcome is unknown for {operation}")]
    DatabaseCommitOutcomeUnknown {
        /// Static operation label safe for production logs.
        operation: &'static str,
    },
    /// An HTTP layer rejected the request before a typed extractor ran.
    #[error("request was rejected with HTTP status {status}")]
    TransportRejected {
        /// Original transport status.
        status: StatusCode,
    },
    /// An unexpected internal operation failed.
    #[error("internal service error in {category}")]
    Internal {
        /// Static subsystem label safe for production logs.
        category: &'static str,
    },
}

/// Public error representation defined by `openapi.yaml`.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    error: ErrorBody,
}

/// Machine-readable error payload nested inside [`ErrorEnvelope`].
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    code: Cow<'static, str>,
    message: Cow<'static, str>,
    request_id: String,
}

impl AppError {
    /// Creates a validation error with a stable code.
    #[must_use]
    pub fn validation(code: impl Into<Cow<'static, str>>) -> Self {
        Self::Validation { code: code.into() }
    }

    /// Creates a malformed-request error with a stable code.
    #[must_use]
    pub fn bad_request(code: impl Into<Cow<'static, str>>) -> Self {
        Self::BadRequest { code: code.into() }
    }

    /// Creates a conflict error with a stable code.
    #[must_use]
    pub fn conflict(code: impl Into<Cow<'static, str>>) -> Self {
        Self::Conflict { code: code.into() }
    }

    fn public_parts(&self) -> (StatusCode, Cow<'static, str>, Cow<'static, str>) {
        match self {
            Self::BadRequest { code } => (
                StatusCode::BAD_REQUEST,
                code.clone(),
                Cow::Borrowed("The request could not be interpreted."),
            ),
            Self::Validation { code } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                code.clone(),
                Cow::Borrowed("The request contains invalid data."),
            ),
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                Cow::Borrowed("unauthenticated"),
                Cow::Borrowed("Authentication is required."),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                Cow::Borrowed("forbidden"),
                Cow::Borrowed("The actor is not authorized for this action."),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                Cow::Borrowed("not_found"),
                Cow::Borrowed("The requested resource was not found."),
            ),
            Self::Conflict { code } => (
                StatusCode::CONFLICT,
                code.clone(),
                Cow::Borrowed("The request conflicts with the current resource state."),
            ),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Cow::Borrowed("payload_too_large"),
                Cow::Borrowed("The request body exceeds the allowed size."),
            ),
            Self::RangeNotSatisfiable { .. } => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                Cow::Borrowed("range_not_satisfiable"),
                Cow::Borrowed("The requested byte range is outside the content."),
            ),
            Self::UploadLimitExhausted { limit, .. } => match limit {
                UploadLimit::DailyUpload => (
                    StatusCode::TOO_MANY_REQUESTS,
                    Cow::Borrowed(UploadLimit::DailyUpload.code()),
                    Cow::Borrowed(
                        "The organization's daily upload allowance is spent. It resets at 00:00 UTC.",
                    ),
                ),
                UploadLimit::Storage => (
                    StatusCode::INSUFFICIENT_STORAGE,
                    Cow::Borrowed(UploadLimit::Storage.code()),
                    Cow::Borrowed("The organization has no storage capacity left."),
                ),
            },
            Self::RateLimited { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                Cow::Borrowed("rate_limited"),
                Cow::Borrowed("Too many requests. Retry later."),
            ),
            Self::Timeout => (
                StatusCode::GATEWAY_TIMEOUT,
                Cow::Borrowed("request_timeout"),
                Cow::Borrowed("The request exceeded its processing deadline."),
            ),
            Self::UnsupportedApiVersion => (
                StatusCode::NOT_ACCEPTABLE,
                Cow::Borrowed("unsupported_api_version"),
                Cow::Borrowed("This build serves no API version the caller supports."),
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                Cow::Borrowed("method_not_allowed"),
                Cow::Borrowed("The HTTP method is not allowed for this route."),
            ),
            Self::DependencyUnavailable { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                Cow::Borrowed("dependency_unavailable"),
                Cow::Borrowed("A required service is temporarily unavailable."),
            ),
            Self::DatabaseCommitOutcomeUnknown { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                Cow::Borrowed("commit_outcome_unknown"),
                Cow::Borrowed(
                    "The operation outcome is temporarily unknown. Retry with the same idempotency key.",
                ),
            ),
            Self::TransportRejected { status } => (
                *status,
                Cow::Borrowed("request_rejected"),
                Cow::Borrowed("The HTTP request was rejected."),
            ),
            Self::Internal { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Cow::Borrowed("internal_error"),
                Cow::Borrowed("An internal service error occurred."),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        match &self {
            Self::DependencyUnavailable { dependency } => {
                error!(error.dependency = *dependency, "required dependency failed");
            }
            Self::DatabaseCommitOutcomeUnknown { operation } => {
                error!(
                    error.operation = *operation,
                    "database commit outcome is unknown"
                );
            }
            Self::Internal { category } => {
                error!(error.category = *category, "internal application failure");
            }
            _ => {}
        }

        let retry_after_seconds = match &self {
            Self::RateLimited {
                retry_after_seconds,
            } => Some(*retry_after_seconds),
            Self::UploadLimitExhausted {
                retry_after_seconds,
                ..
            } => *retry_after_seconds,
            _ => None,
        };
        let unsatisfiable_total_size = match &self {
            Self::RangeNotSatisfiable { total_size } => Some(*total_size),
            _ => None,
        };
        let (status, code, message) = self.public_parts();
        let mut response = (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    request_id: request_id(),
                },
            }),
        )
            .into_response();

        if let Some(retry_after_seconds) = retry_after_seconds
            && let Ok(value) = retry_after_seconds.to_string().parse()
        {
            response
                .headers_mut()
                .insert(http::header::RETRY_AFTER, value);
        }
        if let Some(total_size) = unsatisfiable_total_size
            && let Ok(value) = format!("bytes */{total_size}").parse()
        {
            response
                .headers_mut()
                .insert(http::header::CONTENT_RANGE, value);
        }
        response
    }
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        let category = match error {
            sqlx::Error::PoolTimedOut => "database_pool_timeout",
            sqlx::Error::PoolClosed => "database_pool_closed",
            _ => "database_operation",
        };
        Self::Internal { category }
    }
}

fn request_id() -> String {
    crate::request_context::current_request_id().unwrap_or_else(|| Uuid::now_v7().to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AppError, ErrorBody, ErrorEnvelope};

    #[test]
    fn openapi_envelope_contains_only_the_documented_fields() {
        let value = serde_json::to_value(ErrorEnvelope {
            error: ErrorBody {
                code: "not_found".into(),
                message: "The requested resource was not found.".into(),
                request_id: "request-123".to_owned(),
            },
        });

        assert_eq!(
            value.ok(),
            Some(json!({
                "error": {
                    "code": "not_found",
                    "message": "The requested resource was not found.",
                    "request_id": "request-123"
                }
            }))
        );
    }

    #[test]
    fn internal_categories_never_become_public_messages() {
        let error = AppError::Internal {
            category: "sensitive_storage_detail",
        };
        let (_, code, message) = error.public_parts();

        assert_eq!(code, "internal_error");
        assert!(!message.contains("sensitive_storage_detail"));
    }

    #[test]
    fn unknown_commit_outcomes_are_retryable_and_redacted() {
        let error = AppError::DatabaseCommitOutcomeUnknown {
            operation: "sensitive_internal_operation_label",
        };
        let (status, code, message) = error.public_parts();

        assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(code, "commit_outcome_unknown");
        assert!(message.contains("Retry with the same idempotency key"));
        assert!(!message.contains("sensitive_internal_operation_label"));
    }
}
