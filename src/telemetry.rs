//! Structured process telemetry without secret- or content-bearing payloads.

use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::config::{RuntimeEnvironment, Settings};

mod collector;
pub(crate) use collector::RequestContext;
pub use collector::{configured, flush, record};

/// Carries the caller's opt-out across every asynchronous request step.
pub(crate) async fn scope<T>(
    context: RequestContext,
    future: impl std::future::Future<Output = T>,
) -> T {
    collector::REQUEST.scope(context, future).await
}

/// Installs the process-global tracing subscriber from full settings.
///
/// # Errors
///
/// Returns an error when the filter is invalid or another subscriber has
/// already been installed.
pub fn init(settings: &Settings) -> anyhow::Result<()> {
    init_process(settings.environment, &settings.log_filter)
}

/// Installs telemetry for a process with minimal settings, such as migrations.
///
/// Development and test output is compact and human-readable. Production
/// output is newline-delimited JSON suitable for centralized collection.
///
/// # Errors
///
/// Returns an error when the filter is invalid or another subscriber has
/// already been installed.
pub fn init_process(environment: RuntimeEnvironment, log_filter: &str) -> anyhow::Result<()> {
    collector::init();
    let filter = EnvFilter::try_new(log_filter)?;
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(collector::CollectorLayer);

    match environment {
        RuntimeEnvironment::Development | RuntimeEnvironment::Test => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .compact()
                    .with_ansi(matches!(environment, RuntimeEnvironment::Development))
                    .with_target(true)
                    .with_thread_ids(false),
            )
            .try_init()?,
        RuntimeEnvironment::Production => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_ansi(false)
                    .with_current_span(true)
                    .with_span_list(false)
                    .with_target(true),
            )
            .try_init()?,
    }

    Ok(())
}

/// Correlates authenticated work without changing the original opt-out.
pub(crate) async fn authenticated<T>(
    context: &crate::application::context::ExecutionContext,
    future: impl std::future::Future<Output = T>,
) -> T {
    let environment_id = context
        .testing_environment()
        .map(crate::application::context::TestingEnvironmentContext::id);
    let request = RequestContext {
        enabled: collector::REQUEST
            .try_with(|context| context.enabled)
            .unwrap_or(true),
        testing: environment_id.is_some(),
        environment_id,
        request_id: uuid::Uuid::parse_str(context.request_id()).ok(),
    };
    scope(request, future).await
}

/// Attributes background batches to the testing plane.
pub(crate) async fn testing<T>(future: impl std::future::Future<Output = T>) -> T {
    scope(
        RequestContext {
            enabled: true,
            testing: true,
            request_id: None,
            environment_id: None,
        },
        future,
    )
    .await
}
