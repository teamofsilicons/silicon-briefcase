//! Retired: Briefcase records no telemetry.
//!
//! The types stay so code written against 4.0 still compiles; [`submit`] sends
//! nothing and [`enabled_from_env`] is always false.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Component that observed an event. Remote reports are labeled unverified.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Briefcase API process.
    Backend,
    /// Background job process.
    Worker,
    /// Rust library caller.
    Sdk,
    /// Command-line interface.
    Cli,
    /// Persistent local background service.
    Daemon,
    /// Browser application.
    Web,
}

/// A stage in an operation, suitable for correlating progress and completion.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Operation began.
    Started,
    /// Intermediate work.
    Progress,
    /// Operation succeeded.
    Completed,
    /// Operation failed.
    Failed,
    /// Operation was cancelled.
    Cancelled,
}

/// One self-contained, content-free operational event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Unique identity for this observation.
    pub id: Uuid,
    /// Reporting component.
    pub source: Source,
    /// Bounded machine-readable operation name, never an argument or filename.
    pub operation: String,
    /// Operation lifecycle stage.
    pub stage: Stage,
    /// Legacy field kept for wire compatibility; Briefcase has no testing
    /// environments any more, so clients leave it `false`.
    #[serde(default)]
    pub testing: bool,
    /// Legacy field kept for wire compatibility; always absent.
    pub environment_id: Option<Uuid>,
    /// Correlation with the API response request ID.
    pub request_id: Option<Uuid>,
    /// Time spent on this step.
    pub duration_ms: Option<u64>,
    /// HTTP result, when applicable.
    pub status: Option<u16>,
    /// Retry number or stage number.
    pub attempt: Option<u32>,
    /// Count or transferred byte count, without object names.
    pub count: Option<u64>,
    /// Progress expressed as 0–100.
    pub progress: Option<u8>,
}

impl Event {
    /// Starts a bounded event with a fresh identity.
    #[must_use]
    pub fn new(source: Source, operation: impl Into<String>, stage: Stage) -> Self {
        Self {
            id: Uuid::new_v4(),
            source,
            operation: operation.into(),
            stage,
            testing: false,
            environment_id: None,
            request_id: None,
            duration_ms: None,
            status: None,
            attempt: None,
            count: None,
            progress: None,
        }
    }

    /// Validates the fixed vocabulary boundary before accepting an event.
    #[must_use]
    pub fn valid(&self) -> bool {
        !self.id.is_nil()
            && self.operation.len() <= 96
            && !self.operation.is_empty()
            && self
                .operation
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
            && ![
                "ask_", "slt_", "oat_", "ort_", "sar_", "sap_", "sapr_", "sa_app_", "whsec_",
                "table-", "apikey-", "sscli-", "stk-", "stk_",
            ]
            .iter()
            .any(|prefix| self.operation.contains(prefix))
            && self
                .environment_id
                .is_none_or(|id| !id.is_nil() && self.testing)
            && self.request_id.is_none_or(|id| !id.is_nil())
            && self
                .status
                .is_none_or(|status| (100..=599).contains(&status))
            && self.progress.is_none_or(|value| value <= 100)
    }
}

/// Does nothing: telemetry was removed from Briefcase.
///
/// # Errors
///
/// Never; the event is discarded without being sent.
#[allow(
    clippy::unused_async,
    reason = "kept async for 4.0 callers that await it"
)]
pub async fn submit(_base: &str, _event: &Event) -> crate::Result<()> {
    Ok(())
}

/// Always false: Briefcase records no telemetry, so there is nothing to enable.
/// `BRIEFCASE_TELEMETRY` is ignored.
#[must_use]
pub const fn enabled_from_env() -> bool {
    false
}
