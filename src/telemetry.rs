//! Structured process telemetry without secret- or content-bearing payloads.

use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _, util::SubscriberInitExt as _};

use crate::config::{RuntimeEnvironment, Settings};

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
    let filter = EnvFilter::try_new(log_filter)?;
    let registry = tracing_subscriber::registry().with(filter);

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
