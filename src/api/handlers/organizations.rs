//! Organization controls use the same Accounts session as files.
use super::{
    super::{auth::Action, extract, state::AppState},
    accounts::private_json,
};
use crate::error::AppError;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use secrecy::ExposeSecret as _;
use serde_json::Value;

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let context =
        extract::authenticate(&state, &headers, Action::ReadAccount, "organizations").await?;
    Ok(private_json(
        serde_json::to_value(context.organizations()).map_err(|_| AppError::Internal {
            category: "organizations_response",
        })?,
    ))
}

pub(crate) async fn show(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Result<Response, AppError> {
    validate_org(&org)?;
    let (_, token) =
        extract::authenticate_with_token(&state, &headers, Action::ReadAccount, "organizations")
            .await?;
    let gateway = state.organizations.as_ref().ok_or(AppError::NotFound)?;
    Ok(private_json(
        gateway
            .request(
                token.expose_secret(),
                reqwest::Method::GET,
                &["orgs", &org],
                None,
                None,
            )
            .await?,
    ))
}

pub(crate) async fn action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(org): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, AppError> {
    validate_org(&org)?;
    let (_, token) = extract::authenticate_with_token(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "organizations",
    )
    .await?;
    let key = extract::required_idempotency_key(&headers)?;
    if key.as_str().len() > 200 {
        return Err(AppError::validation(
            "organization_idempotency_key_too_long",
        ));
    }
    let gateway = state.organizations.as_ref().ok_or(AppError::NotFound)?;
    Ok(private_json(
        gateway
            .request(
                token.expose_secret(),
                reqwest::Method::POST,
                &["orgs", &org, "actions"],
                Some(&body),
                Some(key.as_str()),
            )
            .await?,
    ))
}

fn validate_org(org: &str) -> Result<(), AppError> {
    if !(3..=30).contains(&org.len())
        || !org
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
    {
        return Err(AppError::validation("invalid_organization"));
    }
    Ok(())
}
