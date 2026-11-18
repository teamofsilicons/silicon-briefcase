//! Incoming offers for a personal drive or a Silicon managed by its custodian.
use super::{accounts::private_json, drives::managed_account};
use crate::{
    api::{auth::Action, extract, state::AppState},
    error::AppError,
    infrastructure::postgres::share_review::{ReviewDecision, ReviewTarget},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub(crate) struct Page {
    cursor: Option<Uuid>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account): Path<String>,
    Query(page): Query<Page>,
) -> Result<Response, AppError> {
    let ctx = extract::authenticate(
        &state,
        &headers,
        Action::ReadShareAllowances,
        "share-review",
    )
    .await?;
    let (recipient, _) = managed_account(&state, &ctx, &account, false).await?;
    Ok(private_json(
        state
            .content_adapter
            .metadata_repository()
            .share_reviews(&ctx, recipient.as_str(), page.cursor)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Decision {
    decision: ReviewDecision,
}

pub(crate) async fn decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, id)): Path<(String, Uuid)>,
    Json(body): Json<Decision>,
) -> Result<StatusCode, AppError> {
    let ctx = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "share-review",
    )
    .await?;
    let (recipient, _) = managed_account(&state, &ctx, &account, false).await?;
    state
        .content_adapter
        .metadata_repository()
        .decide_share_review(
            &ctx,
            recipient.as_str(),
            ReviewTarget::Offer(id, body.decision),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn add_sender(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, sender)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let ctx = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "share-review",
    )
    .await?;
    let (recipient, _) = managed_account(&state, &ctx, &account, false).await?;
    let sender = state.accounts.resolve(&sender).await?;
    if sender.uuid == recipient.as_str() {
        return Err(AppError::validation("allowance_self"));
    }
    state
        .content_adapter
        .metadata_repository()
        .decide_share_review(&ctx, recipient.as_str(), ReviewTarget::Sender(sender.uuid))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_sender(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((account, sender)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let ctx = extract::authenticate(
        &state,
        &headers,
        Action::ManageShareAllowances,
        "share-review",
    )
    .await?;
    let (recipient, _) = managed_account(&state, &ctx, &account, false).await?;
    let sender = state.accounts.resolve(&sender).await?;
    state
        .content_adapter
        .metadata_repository()
        .remove_review_sender(&ctx, recipient.as_str(), &sender.uuid)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
