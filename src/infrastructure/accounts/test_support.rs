//! Test doubles for Silicon Accounts: an Ed25519 token issuer, signed webhook
//! deliveries, and canned service answers for a `wiremock` stand-in.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::{Value, json};
use silicon_accounts_client::{Jwk, Jwks, sign_webhook};
use time::OffsetDateTime;

/// Signs access tokens the way Silicon Accounts does (`EdDSA`, `kid` header).
pub(crate) struct TestIssuer {
    key: SigningKey,
    kid: String,
    issuer: String,
}

impl TestIssuer {
    /// A deterministic key per `seed`.
    pub(crate) fn new(seed: u8, kid: &str, issuer: &str) -> Self {
        Self {
            key: SigningKey::from_bytes(&[seed; 32]),
            kid: kid.to_owned(),
            issuer: issuer.trim_end_matches('/').to_owned(),
        }
    }

    /// The public key as a JWKS entry.
    pub(crate) fn jwk(&self) -> Jwk {
        Jwk::ed25519(
            self.kid.clone(),
            URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes()),
        )
    }

    /// A key set holding only this issuer's key.
    pub(crate) fn jwks(&self) -> Jwks {
        Jwks::new(vec![self.jwk()])
    }

    /// The JWKS document a Silicon Accounts stand-in serves.
    pub(crate) fn jwks_json(&self) -> Value {
        serde_json::to_value(self.jwks()).unwrap_or(Value::Null)
    }

    /// Signs arbitrary claims.
    pub(crate) fn sign(&self, claims: &Value) -> String {
        let header = json!({ "alg": "EdDSA", "typ": "JWT", "kid": self.kid });
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self.key.sign(signing_input.as_bytes());
        format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }

    /// Claims of a Briefcase access token for `sub`, issued `iat_offset`
    /// seconds from now and valid for `lifetime` seconds after that.
    pub(crate) fn claims(
        &self,
        sub: &str,
        kind: &str,
        id: &str,
        iat_offset: i64,
        lifetime: i64,
    ) -> Value {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        json!({
            "iss": self.issuer,
            "sub": sub,
            "aud": "briefcase",
            "iat": now + iat_offset,
            "nbf": now + iat_offset,
            "exp": now + iat_offset + lifetime,
            "jti": format!("jti-{sub}-{iat_offset}"),
            "kind": kind,
            "id": id,
            "mid": format!("briefcase:{sub}"),
            "fid": format!("fid-{sub}"),
            "scope": "profile",
        })
    }

    /// A valid Briefcase access token issued now.
    pub(crate) fn token(&self, sub: &str, kind: &str, id: &str) -> String {
        self.sign(&self.claims(sub, kind, id, 0, 1_800))
    }
}

/// Headers and body of a signed webhook delivery.
pub(crate) struct SignedDelivery {
    pub(crate) timestamp: String,
    pub(crate) signature: String,
    pub(crate) event_id: String,
    pub(crate) body: String,
}

/// Signs a webhook event the way Silicon Accounts does.
pub(crate) fn signed_delivery(
    secret: &str,
    event_id: &str,
    event_type: &str,
    occurred_at: OffsetDateTime,
    data: &Value,
) -> SignedDelivery {
    let occurred = occurred_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let body = json!({
        "event_id": event_id,
        "type": event_type,
        "occurred_at": occurred,
        "app_id": "briefcase",
        "silicon": null,
        "data": data,
    })
    .to_string();
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    SignedDelivery {
        signature: sign_webhook(secret, timestamp, body.as_bytes()),
        timestamp: timestamp.to_string(),
        event_id: event_id.to_owned(),
        body,
    }
}

/// `GET /v1/accounts/{uuid}` and `/by-id/{id}` answer for a Carbon.
pub(crate) fn carbon_summary(uuid: &str, id: &str) -> Value {
    json!({
        "uuid": uuid, "kind": "carbon", "id": id, "display_name": format!("Carbon {id}"),
        "pfp_url": format!("https://pfp.example/{uuid}.png"), "status": "active",
    })
}

/// `GET /v1/accounts/{uuid}` and `/by-id/{id}` answer for a Silicon.
pub(crate) fn silicon_summary(uuid: &str, id: &str, custodian: (&str, &str)) -> Value {
    json!({
        "uuid": uuid, "kind": "silicon", "id": id, "display_name": format!("Silicon {id}"),
        "pfp_url": format!("https://pfp.example/{uuid}.png"), "status": "active",
        "custodian": { "uuid": custodian.0, "id": custodian.1 },
    })
}

/// `POST /v1/proofs/verify` answer for a valid User verification proof.
pub(crate) fn valid_proof(
    issuer: &str,
    receiver: &str,
    user: (&str, &str, &str),
    scopes: &[&str],
) -> Value {
    let expires = (OffsetDateTime::now_utc() + time::Duration::minutes(10))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    json!({
        "valid": true,
        "proof_id": "01a11435-333a-725d-bb0e-75adde136703",
        "kind": "user_verification",
        "expires_at": expires,
        "issuing_app": { "app_id": issuer, "name": issuer },
        "receiving_app": { "app_id": receiver, "name": receiver },
        "user": { "uuid": user.0, "id": user.1, "kind": user.2, "membership_id": format!("{issuer}:{}", user.0) },
        "scopes": scopes,
    })
}
