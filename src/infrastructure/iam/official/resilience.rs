//! Bounded retries for read-only IAM introspection, never token/proof mutations.
use super::{IamClientError, Operation, sdk_error};
use silicon_iam_client::Error;
use std::{future::Future, time::Duration};
use tokio::time::{Instant, sleep, timeout_at};

const BUDGET: Duration = Duration::from_secs(9);

pub(super) async fn introspect<T, F, Fut>(request: F) -> Result<T, IamClientError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    with_budget(request, BUDGET).await
}

async fn with_budget<T, F, Fut>(mut request: F, budget: Duration) -> Result<T, IamClientError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let deadline = Instant::now() + budget;
    for attempt in 1..=2 {
        let started = Instant::now();
        let result = timeout_at(deadline, request()).await;
        let error = match result {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(error)) => error,
            Err(_) => {
                tracing::warn!(
                    iam.operation = "introspect",
                    iam.failure = "retry_budget_exhausted",
                    iam.attempt = attempt,
                    iam.elapsed_ms =
                        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "IAM introspection deadline exceeded"
                );
                return Err(IamClientError::Unavailable {
                    reason: "retry_budget_exhausted",
                });
            }
        };
        let delay = retry_delay(&error);
        let retry = attempt == 1 && delay.is_some_and(|d| Instant::now() + d < deadline);
        tracing::warn!(
            iam.operation = "introspect",
            iam.failure = failure_class(&error),
            iam.attempt = attempt,
            iam.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            iam.retry = retry,
            "IAM introspection attempt failed"
        );
        if retry {
            log_error(&error);
            sleep(delay.unwrap_or_default()).await;
        } else {
            return Err(sdk_error(error, Operation::Service));
        }
    }
    unreachable!("introspection makes at most two attempts")
}

fn retry_delay(error: &Error) -> Option<Duration> {
    // Small jitter avoids synchronized retries by the three Silicons or other clients.
    let jitter = Duration::from_millis(200 + u64::from(uuid::Uuid::new_v4().as_bytes()[0]));
    match error {
        Error::Transport(e) if e.is_timeout() || e.is_connect() => Some(jitter),
        Error::Api(e) if matches!(e.status, 502..=504) => Some(jitter),
        Error::UnstructuredResponse {
            status: 502..=504, ..
        } => Some(jitter),
        Error::RateLimited { retry_after, .. } => Some((*retry_after).max(jitter)),
        _ => None,
    }
}

pub(super) fn failure_class(error: &Error) -> &'static str {
    match error {
        Error::Transport(e) if e.is_timeout() => "timeout",
        Error::Transport(e) if e.is_connect() => "connection",
        Error::Transport(_) => "transport",
        Error::RateLimited { .. } => "rate_limited",
        Error::Api(e) if e.status == 429 => "rate_limited",
        Error::Api(_) => "upstream_http",
        Error::UnstructuredResponse { .. } => "unstructured_http",
        _ => "invalid_response",
    }
}

pub(super) fn log_error(error: &Error) {
    let (status, request_id) = match error {
        Error::Api(e) | Error::RateLimited { source: e, .. } => {
            (Some(e.status), e.request_id.as_deref())
        }
        Error::UnstructuredResponse { status, request_id } => {
            (Some(*status), request_id.as_deref())
        }
        _ => (None, None),
    };
    // Never log raw errors: URLs, tokens, response bodies and arbitrary upstream text
    // can contain credentials. Accept only a parsed UUID as a correlation identifier.
    let request_id = request_id.and_then(|id| uuid::Uuid::parse_str(id).ok());
    tracing::warn!(iam.failure = failure_class(error), iam.status = ?status,
        iam.request_id = ?request_id, "IAM request failed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn api(status: u16) -> Error {
        Error::Api(Box::new(silicon_iam_client::ApiError {
            status,
            code: "test".into(),
            message: "test".into(),
            details: None,
            request_id: None,
        }))
    }
    #[tokio::test]
    async fn retries_transient_failure_once_and_returns_live_result() {
        let mut attempts = 0;
        let value = introspect(|| {
            attempts += 1;
            std::future::ready(if attempts == 1 {
                Err(api(503))
            } else {
                Ok(false)
            })
        })
        .await
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        // An inactive response after recovery remains inactive, never a cached allow.
        assert!(!value);
        assert_eq!(attempts, 2);
    }
    #[tokio::test]
    async fn persistent_failure_stops_after_two_attempts() {
        let mut attempts = 0;
        let result: Result<(), _> = introspect(|| {
            attempts += 1;
            std::future::ready(Err(api(503)))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 2);
    }
    #[tokio::test]
    async fn denials_and_malformed_success_are_not_retried() {
        for error in [
            api(400),
            api(401),
            api(403),
            Error::Decode("bad body".into()),
        ] {
            let mut error = Some(error);
            let result: Result<(), _> = introspect(|| {
                std::future::ready(Err(error
                    .take()
                    .unwrap_or_else(|| panic!("must not retry"))))
            })
            .await;
            assert!(result.is_err());
        }
    }
    #[tokio::test]
    async fn long_retry_after_is_returned_without_retrying_early() {
        let mut attempts = 0;
        let result: Result<(), _> = introspect(|| {
            attempts += 1;
            let Error::Api(source) = api(429) else {
                unreachable!()
            };
            std::future::ready(Err(Error::RateLimited {
                retry_after: Duration::from_secs(60),
                limit: None,
                remaining: None,
                source,
            }))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }
    #[tokio::test]
    async fn overall_budget_cancels_a_stalled_request() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let result: Result<(), _> = with_budget(
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                std::future::pending()
            },
            Duration::from_millis(20),
        )
        .await;
        assert!(matches!(
            result,
            Err(IamClientError::Unavailable {
                reason: "retry_budget_exhausted"
            })
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn reproduce_original_two_second_503_and_recover_with_four_seconds() -> anyhow::Result<()>
    {
        use axum::response::IntoResponse;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/oauth/introspect"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"active": false}))
                    .set_delay(Duration::from_millis(2500)),
            )
            .expect(2)
            .mount(&server)
            .await;
        let request = silicon_iam_client::models::TokenIntrospectionRequest {
            token: "oat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            token_type_hint: None,
        };
        let old = silicon_iam_client::Client::builder(&server.uri())?
            .timeout(Duration::from_secs(2))
            .auto_update(false)
            .build()?;
        let started = Instant::now();
        let error = old
            .oauth()
            .introspect(&request, Some("tos"))
            .await
            .err()
            .unwrap_or_else(|| panic!("expected timeout"));
        assert_eq!(failure_class(&error), "timeout");
        let response =
            crate::error::AppError::from(sdk_error(error, Operation::Service)).into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        let body = axum::body::to_bytes(response.into_body(), 8192).await?;
        assert!(std::str::from_utf8(&body)?.contains("dependency_unavailable"));
        println!(
            "Reproduced old behavior: HTTP 503 dependency_unavailable after {} ms",
            started.elapsed().as_millis()
        );
        let updated = silicon_iam_client::Client::builder(&server.uri())?
            .timeout(Duration::from_secs(4))
            .auto_update(false)
            .build()?;
        let started = Instant::now();
        let result =
            introspect(|| async { updated.oauth().introspect(&request, Some("tos")).await })
                .await?;
        assert!(!result.active); // Recovery does not turn inactive authority into permission.
        println!(
            "Patched behavior: received IAM response after {} ms; inactive token still denied",
            started.elapsed().as_millis()
        );
        server.verify().await;
        Ok(())
    }

    #[test]
    fn diagnostics_preserve_status_and_uuid_without_upstream_secret_text() {
        use std::io::Write;
        use std::sync::Mutex;
        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|error| panic!("test fixture: {error}"))
                    .extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buffer = Capture(Arc::new(Mutex::new(Vec::new())));
        let output = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || buffer.clone())
            .finish();
        let id = uuid::Uuid::new_v4().to_string();
        let error = Error::Api(Box::new(silicon_iam_client::ApiError {
            status: 503,
            code: "DO_NOT_LOG_THIS".into(),
            message: "DO_NOT_LOG_THIS".into(),
            details: Some(serde_json::json!({"token": "DO_NOT_LOG_THIS"})),
            request_id: Some(id.clone()),
        }));
        tracing::subscriber::with_default(subscriber, || log_error(&error));
        let text = String::from_utf8(
            output
                .0
                .lock()
                .unwrap_or_else(|error| panic!("test fixture: {error}"))
                .clone(),
        )
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        assert!(text.contains("503") && text.contains(&id));
        assert!(!text.contains("DO_NOT_LOG_THIS"));
    }
}
