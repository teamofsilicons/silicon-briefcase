//! Organisation settings use the official SDK; the API enforces administrator rights.
use crate::{App, Result, bad, session};
use axum::{Json, extract::State, http::HeaderMap};
use briefcase_client::{BucketConfiguration, BucketConfigurationStatus, IdempotencyKey};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StorageChange {
    configuration: BucketConfiguration,
    operation_id: Uuid,
}

pub(crate) async fn configure_storage(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<StorageChange>,
) -> Result<Json<BucketConfigurationStatus>> {
    if input.operation_id.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    let client = session::client(&app, &headers).await?;
    Ok(Json(
        client
            .configure_storage_with_key(
                &input.configuration,
                &IdempotencyKey::new(input.operation_id.to_string())?,
            )
            .await?,
    ))
}
