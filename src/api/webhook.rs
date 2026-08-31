//! Signed IAM webhook transport verification.
//!
//! IAM's v1 contract uses `X-Silicon-IAM-Event-ID`,
//! `X-Silicon-IAM-Timestamp`, `X-Silicon-IAM-Key-Version`, and
//! `X-Silicon-IAM-Signature: v1=<lowercase hex>`. The MAC input is the ASCII
//! timestamp, one `.` byte, and the exact raw HTTP body.

use hmac::{Hmac, Mac as _};
use http::{HeaderMap, HeaderName};
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
    let header_event_id = Uuid::parse_str(single_header(headers, &EVENT_ID)?)
        .map_err(|_| AppError::Unauthenticated)?;
    let timestamp_text = single_header(headers, &TIMESTAMP)?;
    let timestamp_seconds = timestamp_text
        .parse::<i64>()
        .map_err(|_| AppError::Unauthenticated)?;
    let now_seconds = OffsetDateTime::now_utc().unix_timestamp();
    if now_seconds.abs_diff(timestamp_seconds) > settings.replay_window.as_secs() {
        return Err(AppError::Unauthenticated);
    }
    let signature_timestamp = OffsetDateTime::from_unix_timestamp(timestamp_seconds)
        .map_err(|_| AppError::Unauthenticated)?;
    let key_version = single_header(headers, &KEY_VERSION)?
        .parse::<u32>()
        .map_err(|_| AppError::Unauthenticated)?;
    if key_version != settings.signing_key_version.get() {
        return Err(AppError::Unauthenticated);
    }
    let signature = parse_signature(single_header(headers, &SIGNATURE)?)?;

    let mut mac = Hmac::<Sha256>::new_from_slice(
        secrecy::ExposeSecret::expose_secret(&settings.signing_secret).as_bytes(),
    )
    .map_err(|_| AppError::Internal {
        category: "webhook_hmac_key",
    })?;
    mac.update(timestamp_text.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&signature)
        .map_err(|_| AppError::Unauthenticated)?;

    let event = serde_json::from_slice::<IamWebhookEvent>(body)
        .map_err(|_| AppError::bad_request("invalid_webhook_event"))?;
    if event.spec_version != CURRENT_SPEC_VERSION {
        return Err(AppError::bad_request("unsupported_webhook_schema"));
    }
    if event.event_id != header_event_id {
        return Err(AppError::Unauthenticated);
    }
    let payload_sha256: [u8; 32] = Sha256::digest(body).into();

    Ok(VerifiedIamWebhook {
        event,
        signature_timestamp,
        payload_sha256,
    })
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
    use std::{
        num::{NonZeroU32, NonZeroUsize},
        time::Duration,
    };

    use hmac::{Hmac, Mac as _};
    use http::HeaderMap;
    use secrecy::SecretString;
    use sha2::Sha256;
    use time::OffsetDateTime;

    use crate::config::WebhookSettings;

    use super::verify;

    fn settings() -> WebhookSettings {
        WebhookSettings {
            signing_secret: SecretString::from("01234567890123456789012345678901".to_owned()),
            signing_key_version: NonZeroU32::MIN,
            replay_window: Duration::from_secs(300),
            max_body_bytes: NonZeroUsize::new(262_144)
                .unwrap_or_else(|| panic!("non-zero test fixture")),
        }
    }

    fn body() -> &'static [u8] {
        br#"{
            "spec_version":"1.0",
            "event_id":"01990a9d-86f1-7000-8000-000000000001",
            "event_type":"organization.membership.updated.v1",
            "occurred_at":"2026-08-31T12:00:00Z",
            "aggregate":{
                "id":"01990a9d-86f1-7000-8000-000000000002",
                "type":"organization_membership",
                "version":4
            },
            "data":{"org_id":"tos"}
        }"#
    }

    fn signed_headers(body: &[u8], event_id: &str) -> HeaderMap {
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(
            secrecy::ExposeSecret::expose_secret(&settings().signing_secret).as_bytes(),
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
    fn signed_event_without_public_organization_handle_fails_closed() {
        let body = br#"{
            "spec_version":"1.0",
            "event_id":"01990a9d-86f1-7000-8000-000000000001",
            "event_type":"organization.membership.updated.v1",
            "occurred_at":"2026-08-31T12:00:00Z",
            "aggregate":{
                "id":"01990a9d-86f1-7000-8000-000000000002",
                "type":"organization_membership",
                "version":4
            },
            "data":{}
        }"#;
        let headers = signed_headers(body, "01990a9d-86f1-7000-8000-000000000001");

        assert!(verify(&headers, body, &settings()).is_err());
    }
}
