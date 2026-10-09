//! Silicon Accounts discovery, the caller's own account, and the webhook
//! Silicon Accounts sends when an account changes.

use axum::{
    Json,
    body::Bytes,
    extract::{State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};
use silicon_accounts_client::{
    EVENT_ID_HEADER, SIGNATURE_HEADER, TIMESTAMP_HEADER, WebhookError, verify_and_parse_webhook,
};
use tracing::info;

use crate::{
    domain::{actor::ActorKind, delegation::DELEGATED_SCOPES},
    error::AppError,
};

use super::super::{auth::Action, extract, state::AppState};

/// Describes Briefcase at Silicon Accounts. Public, so clients can find how
/// to sign in before they hold a token.
pub(crate) async fn discovery(State(state): State<AppState>) -> Response {
    let gateway = state.accounts.gateway();
    private_json(json!({
        "app_id": gateway.app_id(),
        "accounts_url": gateway.issuer(),
        "delegated_scopes": DELEGATED_SCOPES,
    }))
}

/// Returns the caller's own account as Briefcase knows it.
pub(crate) async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (context, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "me").await?;
    let actor = context
        .authorization()
        .account()
        .ok_or(AppError::Internal {
            category: "account_context",
        })?
        .clone();
    let record = extract::scoped(
        &context,
        state.accounts.profile(&actor, Some(token.expose_secret())),
    )
    .await?;
    let custodian = match record.custodian_uuid.as_deref() {
        Some(uuid) => {
            let custodian =
                state
                    .accounts
                    .store()
                    .find(uuid)
                    .await
                    .map_err(|_| AppError::Internal {
                        category: "account_cache",
                    })?;
            json!({
                "uuid": uuid,
                "id": custodian.and_then(|custodian| custodian.public_id),
            })
        }
        None => Value::Null,
    };
    Ok(private_json(json!({
        "uuid": record.uuid,
        "id": record.public_id.or(actor.id),
        "kind": match record.kind {
            ActorKind::Carbon => "carbon",
            ActorKind::Silicon => "silicon",
        },
        "display_name": record.display_name,
        "pfp_url": record.pfp_url,
        "custodian": custodian,
    })))
}

/// Receives a Silicon Accounts webhook delivery.
///
/// The signature is checked over the raw body (5-minute tolerance) before
/// anything is parsed; deliveries are applied once per `event_id`; unknown
/// event types are acknowledged and logged. Answers 204 when handled, 401 for
/// a missing or bad signature, 400 for a signed body that is not an event.
pub(crate) async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, AppError> {
    let body = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge
        } else {
            AppError::bad_request("invalid_webhook_body")
        }
    })?;
    if body.len() > state.webhook_settings.max_body_bytes.get() {
        return Err(AppError::PayloadTooLarge);
    }
    let timestamp = webhook_header(&headers, TIMESTAMP_HEADER)?;
    let signature = webhook_header(&headers, SIGNATURE_HEADER)?;
    let mut refusal = None;
    let mut verified = None;
    for secret in state.webhook_settings.secrets() {
        match verify_and_parse_webhook(
            secret.expose_secret(),
            timestamp,
            signature,
            &body,
            state.webhook_settings.tolerance,
        ) {
            Ok(event) => {
                verified = Some(event);
                break;
            }
            Err(WebhookError::InvalidBody(reason)) => {
                tracing::warn!(event = "accounts_webhook_invalid_body", %reason, "signed webhook body is not an event");
                return Err(AppError::bad_request("webhook_invalid_body"));
            }
            Err(error) => refusal = Some(error),
        }
    }
    let Some(event) = verified else {
        let reason = refusal.map_or_else(String::new, |error| error.to_string());
        tracing::warn!(event = "accounts_webhook_rejected", %reason, "webhook signature refused");
        return Err(AppError::unauthorized(
            "webhook_signature_invalid",
            "The webhook signature does not verify with Briefcase's Silicon Accounts webhook secret, or its timestamp is more than 5 minutes from this clock.",
        ));
    };
    if let Some(header_id) = optional_header(&headers, EVENT_ID_HEADER)
        && header_id != event.event_id
    {
        return Err(AppError::bad_request("webhook_event_id_mismatch"));
    }
    let outcome = state.accounts.apply_webhook(&event).await?;
    info!(
        event = "accounts_webhook_processed",
        event_id = %event.event_id,
        event_type = %event.event_type,
        outcome = ?outcome,
        "Silicon Accounts webhook processed"
    );
    Ok(StatusCode::NO_CONTENT)
}

fn webhook_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<&'a str, AppError> {
    optional_header(headers, name).ok_or_else(|| {
        AppError::unauthorized(
            "webhook_signature_missing",
            format!("The webhook request has no {name} header; only Silicon Accounts deliveries are accepted here."),
        )
    })
}

fn optional_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

fn private_json(body: Value) -> Response {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
