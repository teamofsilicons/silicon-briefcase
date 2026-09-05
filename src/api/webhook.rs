//! Signed IAM webhook transport verification.
//!
//! IAM's v1 contract uses `X-Silicon-IAM-Event-ID`,
//! `X-Silicon-IAM-Timestamp`, `X-Silicon-IAM-Key-Version`, and
//! `X-Silicon-IAM-Signature: v1=<lowercase hex>`. The MAC input is the ASCII
//! timestamp, one `.` byte, and the exact raw HTTP body.

use hmac::{Hmac, Mac as _};
use http::{HeaderMap, HeaderName};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::webhook::{IamWebhookEvent, VerifiedIamWebhook},
    config::WebhookSettings,
    error::AppError,
};

const EVENT_ID: HeaderName = HeaderName::from_static("x-silicon-iam-event-id");
const KEY_VERSION: HeaderName = HeaderName::from_static("x-silicon-iam-key-version");
const SIGNATURE: HeaderName = HeaderName::from_static("x-silicon-iam-signature");
const TIMESTAMP: HeaderName = HeaderName::from_static("x-silicon-iam-timestamp");
const CURRENT_SPEC_VERSION: &str = "1.0";

/// Verifies replay age and HMAC before parsing an IAM webhook body.
///
/// # Errors
///
/// Returns an opaque authentication error for a missing, malformed, stale, or
/// invalid signature and a bad-request error for an authenticated invalid event.
pub fn verify(
    headers: &HeaderMap,
    body: &[u8],
    settings: &WebhookSettings,
) -> Result<VerifiedIamWebhook, AppError> {
    if body.len() > settings.max_body_bytes.get() {
        return Err(AppError::bad_request("webhook_body_too_large"));
    }
    let header_event_id_text = single_header(headers, &EVENT_ID)?;
    let header_event_id = Uuid::parse_str(header_event_id_text)
        .ok()
        .filter(|event_id| event_id.to_string() == header_event_id_text)
        .ok_or(AppError::Unauthenticated)?;
    let timestamp_text = single_header(headers, &TIMESTAMP)?;
    let timestamp_seconds = canonical_positive_i64(timestamp_text)?;
    let now_seconds = OffsetDateTime::now_utc().unix_timestamp();
    if now_seconds.abs_diff(timestamp_seconds) > settings.replay_window.as_secs() {
        return Err(AppError::Unauthenticated);
    }
    let signature_timestamp = OffsetDateTime::from_unix_timestamp(timestamp_seconds)
        .map_err(|_| AppError::Unauthenticated)?;
    let key_version = canonical_positive_i64(single_header(headers, &KEY_VERSION)?)?;
    let signing_secret = settings
        .signing_secrets
        .get(&key_version)
        .ok_or(AppError::Unauthenticated)?;
    let signature = parse_signature(single_header(headers, &SIGNATURE)?)?;

    let mut mac = Hmac::<Sha256>::new_from_slice(
        secrecy::ExposeSecret::expose_secret(signing_secret).as_bytes(),
    )
    .map_err(|_| AppError::Internal {
        category: "webhook_hmac_key",
    })?;
    mac.update(timestamp_text.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| AppError::Unauthenticated)?;

    let (event, testing_key) = parse_authenticated_event(body)?;
    if event.spec_version != CURRENT_SPEC_VERSION {
        return Err(AppError::bad_request("unsupported_webhook_schema"));
    }
    if event.event_id != header_event_id {
        return Err(AppError::Unauthenticated);
    }
    let payload_sha256: [u8; 32] = Sha256::digest(body).into();

    Ok(VerifiedIamWebhook::new(
        event,
        signature_timestamp,
        payload_sha256,
        testing_key,
    ))
}

fn canonical_positive_i64(value: &str) -> Result<i64, AppError> {
    value
        .parse::<i64>()
        .ok()
        .filter(|parsed| *parsed > 0 && parsed.to_string() == value)
        .ok_or(AppError::Unauthenticated)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestingEnvelope {
    test: TestingEvent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestingEvent {
    testing_key: String,
    metadata: TestingMetadata,
    data: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestingMetadata {
    spec_version: Value,
    event_id: Uuid,
    event_type: String,
    occurred_at: Value,
    organization_id: RequiredNullableUuid,
    aggregate: Value,
}

#[derive(Deserialize)]
#[serde(transparent)]
struct RequiredNullableUuid(Option<Uuid>);

fn parse_authenticated_event(
    body: &[u8],
) -> Result<(IamWebhookEvent, Option<SecretString>), AppError> {
    let value = serde_json::from_slice::<Value>(body)
        .map_err(|_| AppError::bad_request("invalid_webhook_event"))?;
    let object = value
        .as_object()
        .ok_or_else(|| AppError::bad_request("invalid_webhook_event"))?;
    if object.contains_key("test") {
        if object.len() != 1 {
            return Err(AppError::bad_request("invalid_webhook_event"));
        }
        let envelope = serde_json::from_value::<TestingEnvelope>(value)
            .map_err(|_| AppError::bad_request("invalid_webhook_event"))?;
        if !valid_environment_key(&envelope.test.testing_key) {
            return Err(AppError::bad_request("invalid_webhook_event"));
        }
        let metadata = envelope.test.metadata;
        let normalized = serde_json::json!({
            "spec_version": metadata.spec_version,
            "event_id": metadata.event_id,
            "event_type": metadata.event_type,
            "occurred_at": metadata.occurred_at,
            "organization_id": metadata.organization_id.0,
            "aggregate": metadata.aggregate,
            "data": envelope.test.data,
        });
        let event = serde_json::from_value::<IamWebhookEvent>(normalized)
            .map_err(|_| AppError::bad_request("invalid_webhook_event"))?;
        Ok((event, Some(SecretString::from(envelope.test.testing_key))))
    } else {
        let event = serde_json::from_value::<IamWebhookEvent>(value)
            .map_err(|_| AppError::bad_request("invalid_webhook_event"))?;
        Ok((event, None))
    }
}

fn valid_environment_key(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn single_header<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Result<&'a str, AppError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next().ok_or(AppError::Unauthenticated)?;
    if values.next().is_some() {
        return Err(AppError::Unauthenticated);
    }
    first.to_str().map_err(|_| AppError::Unauthenticated)
}

fn parse_signature(value: &str) -> Result<[u8; 32], AppError> {
    let encoded = value.strip_prefix("v1=").ok_or(AppError::Unauthenticated)?;
    if encoded.len() != 64
        || encoded
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::Unauthenticated);
    }
    let mut signature = [0_u8; 32];
    hex::decode_to_slice(encoded, &mut signature).map_err(|_| AppError::Unauthenticated)?;
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, num::NonZeroUsize, time::Duration};

    use hmac::{Hmac, Mac as _};
    use http::HeaderMap;
    use secrecy::SecretString;
    use sha2::Sha256;
    use time::OffsetDateTime;

    use crate::config::WebhookSettings;

    use super::{canonical_positive_i64, verify};

    fn settings() -> WebhookSettings {
        WebhookSettings {
            signing_secrets: BTreeMap::from([(
                1,
                SecretString::from("01234567890123456789012345678901".to_owned()),
            )]),
            replay_window: Duration::from_secs(300),
            max_body_bytes: NonZeroUsize::new(262_144)
                .unwrap_or_else(|| panic!("non-zero test fixture")),
        }
    }

    #[test]
    fn timestamp_and_key_version_headers_are_canonical_decimal() {
        assert_eq!(canonical_positive_i64("1").ok(), Some(1));
        assert_eq!(
            canonical_positive_i64(&i64::MAX.to_string()).ok(),
            Some(i64::MAX)
        );
        assert!(canonical_positive_i64("0").is_err());
        assert!(canonical_positive_i64("-1").is_err());
        assert!(canonical_positive_i64("01").is_err());
        assert!(canonical_positive_i64("+1").is_err());
        assert!(canonical_positive_i64("9223372036854775808").is_err());
    }

    fn body() -> &'static [u8] {
        br#"{
            "spec_version":"1.0",
            "event_id":"01990a9d-86f1-7000-8000-000000000001",
            "event_type":"organization.membership.updated.v1",
            "occurred_at":"2026-08-31T12:00:00Z",
            "organization_id":"01990a9d-86f1-7000-8000-000000000005",
            "aggregate":{
                "id":"01990a9d-86f1-7000-8000-000000000002",
                "type":"organization_membership",
                "version":4
            },
            "data":{"current":{"members":[{
                "resource":{
                    "id":"01990a9d-86f1-7000-8000-000000000003",
                    "type":"organization_membership",
                    "version":4,
                    "status":"active",
                    "principal_id":"01990a9d-86f1-7000-8000-000000000004",
                    "principal_type":"carbon"
                },
                "principal":{"public_id":"carbon-a"},
                "organization":{"org_id":"tos"},
                "membership":{"authorization_epoch":7,"tags":["engineering"]},
                "roles":{"org_role":"member"}
            }]}}
        }"#
    }

    fn testing_body() -> &'static [u8] {
        br#"{
            "test":{
                "testing_key":"A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6",
                "metadata":{
                    "spec_version":"1.0",
                    "event_id":"01990a9d-86f1-7000-8000-000000000001",
                    "event_type":"organization.membership.updated.v1",
                    "occurred_at":"2026-08-31T12:00:00Z",
                    "organization_id":"01990a9d-86f1-7000-8000-000000000005",
                    "aggregate":{
                        "id":"01990a9d-86f1-7000-8000-000000000002",
                        "type":"organization_membership",
                        "version":4
                    }
                },
                "data":{"current":{"members":[{
                    "resource":{
                        "id":"01990a9d-86f1-7000-8000-000000000003",
                        "type":"organization_membership",
                        "version":4,
                        "status":"active",
                        "principal_id":"01990a9d-86f1-7000-8000-000000000004",
                        "principal_type":"carbon"
                    },
                    "principal":{"public_id":"carbon-a"},
                    "organization":{"org_id":"tos"},
                    "membership":{"authorization_epoch":7,"tags":["engineering"]},
                    "roles":{"org_role":"member"}
                }]}}
            }
        }"#
    }

    fn signed_headers(body: &[u8], event_id: &str) -> HeaderMap {
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(
            secrecy::ExposeSecret::expose_secret(
                settings()
                    .signing_secrets
                    .get(&1)
                    .unwrap_or_else(|| panic!("test fixture must include key version 1")),
            )
            .as_bytes(),
        )
        .unwrap_or_else(|error| panic!("test fixture: {error}"));
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let signature = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-silicon-iam-event-id",
            event_id
                .parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );
        headers.insert(
            "x-silicon-iam-timestamp",
            timestamp
                .parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );
        headers.insert(
            "x-silicon-iam-key-version",
            "1".parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );
        headers.insert(
            "x-silicon-iam-signature",
            signature
                .parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );
        headers
    }

    #[test]
    fn authenticates_the_exact_raw_body() {
        let headers = signed_headers(body(), "01990a9d-86f1-7000-8000-000000000001");
        let verified = verify(&headers, body(), &settings());

        assert!(verified.is_ok());
    }

    #[test]
    fn authenticates_and_normalizes_the_exact_wrapped_test_body() {
        let key = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6";
        let headers = signed_headers(testing_body(), "01990a9d-86f1-7000-8000-000000000001");
        let verified = verify(&headers, testing_body(), &settings())
            .unwrap_or_else(|error| panic!("signed test event should verify: {error}"));

        assert!(verified.is_testing());
        assert!(verified.testing_key_matches(&SecretString::from(key.to_owned())));
        assert!(!verified.testing_key_matches(&SecretString::from("Z".repeat(32))));
        assert_eq!(
            verified
                .event
                .org_id
                .as_ref()
                .map(crate::domain::actor::OrganizationId::as_str),
            Some("tos")
        );
        assert!(!format!("{verified:?}").contains(key));
    }

    #[test]
    fn test_envelope_is_strict_and_requires_a_well_formed_root_key() {
        let mut value: serde_json::Value = serde_json::from_slice(testing_body())
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        value["test"]["testing_key"] = serde_json::json!("invalid-key");
        let invalid_key =
            serde_json::to_vec(&value).unwrap_or_else(|error| panic!("test fixture: {error}"));
        let headers = signed_headers(&invalid_key, "01990a9d-86f1-7000-8000-000000000001");
        assert!(verify(&headers, &invalid_key, &settings()).is_err());

        let mut value: serde_json::Value = serde_json::from_slice(testing_body())
            .unwrap_or_else(|error| panic!("test fixture: {error}"));
        value["extra"] = serde_json::json!(true);
        let extra =
            serde_json::to_vec(&value).unwrap_or_else(|error| panic!("test fixture: {error}"));
        let headers = signed_headers(&extra, "01990a9d-86f1-7000-8000-000000000001");
        assert!(verify(&headers, &extra, &settings()).is_err());
    }

    #[test]
    fn a_valid_signature_does_not_cover_a_modified_body() {
        let headers = signed_headers(body(), "01990a9d-86f1-7000-8000-000000000001");

        assert!(verify(&headers, b"{}", &settings()).is_err());
    }

    #[test]
    fn event_id_header_must_match_the_signed_envelope() {
        let headers = signed_headers(body(), "01990a9d-86f1-7000-8000-000000000099");

        assert!(verify(&headers, body(), &settings()).is_err());
    }

    #[test]
    fn signing_key_version_must_match_configuration() {
        let mut headers = signed_headers(body(), "01990a9d-86f1-7000-8000-000000000001");
        headers.insert(
            "x-silicon-iam-key-version",
            "8".parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );

        assert!(verify(&headers, body(), &settings()).is_err());
    }

    #[test]
    fn retained_previous_signing_key_version_is_accepted() {
        let mut settings = settings();
        settings.signing_secrets.insert(
            8,
            SecretString::from("01234567890123456789012345678901".to_owned()),
        );
        let mut headers = signed_headers(body(), "01990a9d-86f1-7000-8000-000000000001");
        headers.insert(
            "x-silicon-iam-key-version",
            "8".parse()
                .unwrap_or_else(|error| panic!("test fixture: {error}")),
        );

        assert!(verify(&headers, body(), &settings).is_ok());
    }

    #[test]
    fn signed_future_event_without_a_public_organization_can_be_ignored() {
        let body = br#"{
            "spec_version":"1.0",
            "event_id":"01990a9d-86f1-7000-8000-000000000001",
            "event_type":"organization.future_event.v1",
            "occurred_at":"2026-08-31T12:00:00Z",
            "aggregate":{
                "id":"01990a9d-86f1-7000-8000-000000000002",
                "type":"organization_membership",
                "version":4
            },
            "data":{}
        }"#;
        let headers = signed_headers(body, "01990a9d-86f1-7000-8000-000000000001");
        let verified = verify(&headers, body, &settings())
            .unwrap_or_else(|error| panic!("unknown signed event should parse: {error}"));

        assert!(verified.event.org_id.is_none());
    }
}
