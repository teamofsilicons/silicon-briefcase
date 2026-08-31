//! IAM webhook projection and deduplication use case.

use async_trait::async_trait;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{domain::actor::OrganizationId, error::AppError};

/// Versioned IAM event accepted after transport signature verification.
#[derive(Clone, Debug)]
pub struct IamWebhookEvent {
    /// Globally unique delivery/event identifier.
    pub event_id: Uuid,
    /// Published semantic envelope version, such as `1.0`.
    pub spec_version: String,
    /// Normalized major schema version retained by the projection repository.
    pub schema_version: u16,
    /// Monotonic IAM aggregate version.
    pub aggregate_version: u64,
    /// IAM aggregate identifier from the signed envelope.
    pub aggregate_id: Uuid,
    /// IAM aggregate type from the signed envelope.
    pub aggregate_type: String,
    /// Stable IAM event type.
    pub event_type: String,
    /// Public IAM organization handle required from `data.org_id`.
    pub org_id: OrganizationId,
    /// Time IAM committed the source change.
    pub occurred_at: OffsetDateTime,
    /// Minimal event-specific projection payload.
    pub data: Value,
}

#[derive(Deserialize)]
struct WireIamWebhookEvent {
    spec_version: String,
    event_id: Uuid,
    event_type: String,
    #[serde(with = "time::serde::rfc3339")]
    occurred_at: OffsetDateTime,
    aggregate: WireIamAggregate,
    data: Value,
}

#[derive(Deserialize)]
struct WireIamAggregate {
    id: Uuid,
    #[serde(rename = "type")]
    aggregate_type: String,
    version: u64,
}

impl<'de> Deserialize<'de> for IamWebhookEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = WireIamWebhookEvent::deserialize(deserializer)?;
        let org_id = wire
            .data
            .get("org_id")
            .and_then(Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("IAM webhook data.org_id is required"))?;
        let org_id = OrganizationId::new(org_id.to_owned())
            .map_err(|_| serde::de::Error::custom("IAM webhook data.org_id is invalid"))?;
        let schema_version = u16::from(wire.spec_version == "1.0");

        Ok(Self {
            event_id: wire.event_id,
            spec_version: wire.spec_version,
            schema_version,
            aggregate_version: wire.aggregate.version,
            aggregate_id: wire.aggregate.id,
            aggregate_type: wire.aggregate.aggregate_type,
            event_type: wire.event_type,
            org_id,
            occurred_at: wire.occurred_at,
            data: wire.data,
        })
    }
}

/// Event plus transport evidence established by the HMAC verifier.
#[derive(Clone, Debug)]
pub struct VerifiedIamWebhook {
    /// Parsed versioned event.
    pub event: IamWebhookEvent,
    /// Timestamp covered by the verified signature.
    pub signature_timestamp: OffsetDateTime,
    /// SHA-256 of the exact signed request body.
    pub payload_sha256: [u8; 32],
}

/// Durable result of applying an at-least-once IAM delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebhookApplyOutcome {
    /// Event was new and its projection transition was applied.
    Applied,
    /// Event ID had already been processed.
    Duplicate,
    /// Event was authenticated but older than the current aggregate projection.
    Stale,
    /// Event type is not yet consumed; receipt was retained for reconciliation.
    Ignored,
}

/// Transactional IAM projection boundary.
#[async_trait]
pub trait IamWebhookRepository: Send + Sync {
    /// Deduplicates the event and applies its projection atomically.
    async fn apply_iam_event(
        &self,
        webhook: &VerifiedIamWebhook,
    ) -> Result<WebhookApplyOutcome, AppError>;
}
