//! IAM webhook projection and deduplication use case.

use std::fmt;

use async_trait::async_trait;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    domain::actor::{OrganizationId, is_canonical_iam_organization_id},
    error::AppError,
};

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
    /// IAM's internal organization UUID, used only to route scoped tombstones.
    pub organization_id: Option<Uuid>,
    /// Public IAM organization handle derived from the scoped current snapshot.
    ///
    /// This is optional so an authenticated event type introduced after this
    /// Briefcase release can still be acknowledged and ignored safely.
    pub org_id: Option<OrganizationId>,
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
    #[serde(default)]
    organization_id: Option<Uuid>,
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
        if wire.aggregate.version == 0 {
            return Err(serde::de::Error::custom(
                "IAM webhook aggregate version must be positive",
            ));
        }
        if !valid_event_type(&wire.event_type) {
            return Err(serde::de::Error::custom(
                "IAM webhook event type is malformed",
            ));
        }
        if !wire.data.is_object() {
            return Err(serde::de::Error::custom(
                "IAM webhook data must be an object",
            ));
        }
        let org_id = public_organization_id(&wire.data)
            .map(|value| {
                if !is_canonical_iam_organization_id(value) {
                    return Err(());
                }
                OrganizationId::new(value.to_owned()).map_err(|_| ())
            })
            .transpose()
            .map_err(|()| serde::de::Error::custom("IAM webhook organization handle is invalid"))?;
        let schema_version = u16::from(wire.spec_version == "1.0");

        Ok(Self {
            event_id: wire.event_id,
            spec_version: wire.spec_version,
            schema_version,
            aggregate_version: wire.aggregate.version,
            aggregate_id: wire.aggregate.id,
            aggregate_type: wire.aggregate.aggregate_type,
            event_type: wire.event_type,
            organization_id: wire.organization_id,
            org_id,
            occurred_at: wire.occurred_at,
            data: wire.data,
        })
    }
}

fn valid_event_type(value: &str) -> bool {
    let mut segments = value.split('.');
    let Some(version) = segments
        .next_back()
        .and_then(|value| value.strip_prefix('v'))
    else {
        return false;
    };
    if version.is_empty()
        || version.starts_with('0')
        || !version.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    let mut names = 0_usize;
    for segment in segments {
        if segment.is_empty()
            || !segment
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_lowercase)
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return false;
        }
        names += 1;
    }
    names >= 2
}

fn public_organization_id(data: &Value) -> Option<&str> {
    data.get("org_id")
        .and_then(Value::as_str)
        .or_else(|| {
            data.pointer("/current/organization/org_id")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            data.pointer("/current/members")
                .and_then(Value::as_array)
                .and_then(|members| {
                    members.iter().find_map(|member| {
                        member
                            .pointer("/organization/org_id")
                            .and_then(Value::as_str)
                    })
                })
        })
}

/// Event plus transport evidence established by the HMAC verifier.
#[derive(Clone)]
pub struct VerifiedIamWebhook {
    /// Parsed versioned event.
    pub event: IamWebhookEvent,
    /// Timestamp covered by the verified signature.
    pub signature_timestamp: OffsetDateTime,
    /// SHA-256 of the exact signed request body.
    pub payload_sha256: [u8; 32],
    /// Test-plane root key retained only for opaque, constant-time routing.
    testing_key: Option<SecretString>,
}

impl VerifiedIamWebhook {
    /// Constructs transport evidence after exact-byte signature verification.
    #[must_use]
    pub(crate) const fn new(
        event: IamWebhookEvent,
        signature_timestamp: OffsetDateTime,
        payload_sha256: [u8; 32],
        testing_key: Option<SecretString>,
    ) -> Self {
        Self {
            event,
            signature_timestamp,
            payload_sha256,
            testing_key,
        }
    }

    /// Reports whether IAM sent the explicit testing-environment envelope.
    #[must_use]
    pub const fn is_testing(&self) -> bool {
        self.testing_key.is_some()
    }

    /// Compares the authenticated test key with an expected key without
    /// exposing the received root credential.
    #[must_use]
    pub fn testing_key_matches(&self, expected: &SecretString) -> bool {
        self.testing_key.as_ref().is_some_and(|received| {
            bool::from(
                received
                    .expose_secret()
                    .as_bytes()
                    .ct_eq(expected.expose_secret().as_bytes()),
            )
        })
    }
}

impl fmt::Debug for VerifiedIamWebhook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedIamWebhook")
            .field("event", &self.event)
            .field("signature_timestamp", &self.signature_timestamp)
            .field("payload_sha256", &self.payload_sha256)
            .field(
                "testing_key",
                &self.testing_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
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
        testing_environment: Option<crate::application::context::TestingEnvironmentContext>,
    ) -> Result<WebhookApplyOutcome, AppError>;
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use time::OffsetDateTime;

    use super::{IamWebhookEvent, VerifiedIamWebhook, valid_event_type};

    #[test]
    fn webhook_event_names_use_the_open_versioned_vocabulary() {
        assert!(valid_event_type("organization.membership.updated.v1"));
        assert!(valid_event_type("future.resource.changed.v7"));
        assert!(!valid_event_type("future.v1"));
        assert!(!valid_event_type("future.Resource.changed.v1"));
        assert!(!valid_event_type("future.resource.changed.v01"));
    }

    #[test]
    fn testing_keys_are_only_available_through_constant_time_matching() {
        let event = serde_json::from_value::<IamWebhookEvent>(serde_json::json!({
            "spec_version": "1.0",
            "event_id": "01990a9d-86f1-7000-8000-000000000001",
            "event_type": "organization.membership.created.v1",
            "occurred_at": "2026-09-04T00:00:00Z",
            "organization_id": null,
            "aggregate": {
                "type": "membership",
                "id": "01990a9d-86f1-7000-8000-000000000002",
                "version": 1
            },
            "data": {}
        }))
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        let key = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6";
        let verified = VerifiedIamWebhook::new(
            event,
            OffsetDateTime::now_utc(),
            [0; 32],
            Some(SecretString::from(key.to_owned())),
        );

        assert!(verified.is_testing());
        assert!(verified.testing_key_matches(&SecretString::from(key.to_owned())));
        assert!(!verified.testing_key_matches(&SecretString::from("Z".repeat(32))));
        assert!(!format!("{verified:?}").contains(key));
    }
}
