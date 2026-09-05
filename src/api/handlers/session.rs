//! Stateless IAM Application-session broker handlers.

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{domain::actor::ActorKind, error::AppError, infrastructure::iam::IamApplicationTokens};

use super::super::{extract, state::AppState};

/// Single-use IAM Application login credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShortLivedTokenRequest {
    slt: SecretString,
}

/// Rotating IAM Application refresh credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefreshTokenRequest {
    refresh_token: SecretString,
}

/// IAM actor represented by the issued session.
#[derive(Serialize)]
struct SessionActor {
    principal_id: Uuid,
    #[serde(rename = "type")]
    actor_type: ActorKind,
    public_id: String,
}

/// Token response mirrored from IAM's Application OAuth contract.
#[derive(Serialize)]
struct SessionTokens {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in: u64,
    scope: String,
    actor: SessionActor,
    org_id: Option<String>,
}

/// Exchanges a single-use short-lived token for an Application session.
pub(crate) async fn exchange_slt(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ShortLivedTokenRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = extract::json(body)?;
    let idempotency_key = auth_idempotency_key(&headers)?;
    let access = extract::optional_testing_access(&state, &headers).await?;
    let _fence = extract::testing_use_fence(&state, access.as_ref()).await?;
    let environment = access
        .as_ref()
        .map(extract::iam_environment_credential)
        .transpose()?;
    let tokens = state
        .iam
        .exchange_short_lived_token(&body.slt, idempotency_key, environment.as_ref())
        .await?;
    extract::touch_testing_access(&state, access.as_ref()).await?;
    Ok(token_response(&tokens))
}

/// Rotates an IAM Application refresh token exactly once.
pub(crate) async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<RefreshTokenRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let body = extract::json(body)?;
    let idempotency_key = auth_idempotency_key(&headers)?;
    let access = extract::optional_testing_access(&state, &headers).await?;
    let _fence = extract::testing_use_fence(&state, access.as_ref()).await?;
    let environment = access
        .as_ref()
        .map(extract::iam_environment_credential)
        .transpose()?;
    let tokens = state
        .iam
        .refresh_application_session(&body.refresh_token, idempotency_key, environment.as_ref())
        .await?;
    extract::touch_testing_access(&state, access.as_ref()).await?;
    Ok(token_response(&tokens))
}

fn auth_idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    let key = extract::required_idempotency_key(headers)?;
    if key.as_str().len() < 16 || !key.as_str().bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(AppError::bad_request("invalid_idempotency_key"));
    }
    // The value is borrowed from the header map, whereas the validated wrapper
    // owns a copy. Re-read the one validated header so the IAM call can borrow
    // it without retaining credential-adjacent request state.
    headers
        .get(header::HeaderName::from_static("idempotency-key"))
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::bad_request("invalid_idempotency_key"))
}

fn token_response(tokens: &IamApplicationTokens) -> Response {
    let actor = tokens.actor();
    let body = SessionTokens {
        access_token: tokens.access_token().expose_secret().to_owned(),
        refresh_token: tokens.refresh_token().expose_secret().to_owned(),
        token_type: "Bearer",
        expires_in: tokens.expires_in_seconds(),
        scope: tokens.scope().to_owned(),
        actor: SessionActor {
            principal_id: tokens.principal_id(),
            actor_type: actor.kind(),
            public_id: actor.id().as_str().to_owned(),
        },
        org_id: tokens
            .organization_id()
            .map(|organization| organization.as_str().to_owned()),
    };
    // The official IAM SDK preserves retry keys but does not expose upstream
    // replay headers. Do not claim a replay was false when it is unknown.
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(body),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::auth_idempotency_key;

    #[test]
    fn application_session_idempotency_keys_are_at_least_sixteen_bytes() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", HeaderValue::from_static("too-short"));
        assert!(auth_idempotency_key(&headers).is_err());
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("application-login-123"),
        );
        assert_eq!(
            auth_idempotency_key(&headers).ok(),
            Some("application-login-123")
        );
    }
}
