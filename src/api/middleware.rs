//! Shared HTTP correlation, deadline, and error-normalization middleware.

use std::{any::Any, time::Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::{IntoResponse as _, Response},
};
use tracing::{Instrument as _, info, info_span};
use uuid::Uuid;

use crate::{error::AppError, request_context};

/// Runs one request inside a validated request-ID scope and tracing span.
pub async fn request_scope(mut request: Request, next: Next) -> Response {
    let request_id = validated_request_id(&request).unwrap_or_else(|| Uuid::now_v7().to_string());
    if let Ok(value) = request_id.parse() {
        request.headers_mut().insert(request_id_header(), value);
    }
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("<unmatched>", MatchedPath::as_str)
        .to_owned();
    let started_at = Instant::now();
    let span = info_span!(
        "http.request",
        request_id = %request_id,
        method = %method,
        route = %route,
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    );

    let future = async move { normalize_error_response(next.run(request).await) };
    let mut response = request_context::scope(request_id.clone(), future)
        .instrument(span.clone())
        .await;
    span.record("status", response.status().as_u16());
    let latency_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    span.record("latency_ms", latency_ms);
    info!(parent: &span, "HTTP request completed");
    if let Ok(value) = request_id.parse() {
        response.headers_mut().insert(request_id_header(), value);
    }
    response
}

/// Enforces one route group's complete processing deadline.
pub async fn enforce_timeout(
    timeout: std::time::Duration,
    request: Request,
    next: Next,
) -> Response {
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => AppError::Timeout.into_response(),
    }
}

/// Converts a caught handler panic to the stable public error envelope.
pub fn handle_panic(_panic: Box<dyn Any + Send + 'static>) -> Response {
    AppError::Internal {
        category: "request_handler_panic",
    }
    .into_response()
}

fn validated_request_id(request: &Request) -> Option<String> {
    let mut values = request.headers().get_all(request_id_header()).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let value = value.to_str().ok()?;
    let valid = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    valid.then(|| value.to_owned())
}

fn normalize_error_response(response: Response) -> Response {
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return response;
    }
    let is_json = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if is_json {
        return response;
    }

    let replacement = match response.status() {
        http::StatusCode::NOT_FOUND => AppError::NotFound.into_response(),
        http::StatusCode::METHOD_NOT_ALLOWED => AppError::MethodNotAllowed.into_response(),
        http::StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge.into_response(),
        http::StatusCode::REQUEST_TIMEOUT | http::StatusCode::GATEWAY_TIMEOUT => {
            AppError::Timeout.into_response()
        }
        status => AppError::TransportRejected { status }.into_response(),
    };
    let (original_parts, _) = response.into_parts();
    let (mut replacement_parts, replacement_body) = replacement.into_parts();
    replacement_parts.status = original_parts.status;
    for (name, value) in &original_parts.headers {
        if name != http::header::CONTENT_TYPE && name != http::header::CONTENT_LENGTH {
            replacement_parts
                .headers
                .append(name.clone(), value.clone());
        }
    }
    Response::from_parts(replacement_parts, replacement_body)
}

const fn request_id_header() -> http::HeaderName {
    http::HeaderName::from_static("x-request-id")
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::validated_request_id;

    #[test]
    fn accepts_only_log_safe_request_identifiers() {
        let safe = axum::http::Request::builder()
            .header("x-request-id", "req_123-ABC")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let unsafe_value = axum::http::Request::builder()
            .header("x-request-id", "contains spaces")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("test fixture: {error}"));

        assert_eq!(validated_request_id(&safe).as_deref(), Some("req_123-ABC"));
        assert_eq!(validated_request_id(&unsafe_value), None);
    }
}
