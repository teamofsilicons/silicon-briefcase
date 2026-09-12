//! Production-plane administration of disposable environments via the official SDK.
use crate::{App, Result, bad, session};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use briefcase_client::{
    ApplicationId, Client, IamApplicationSecret, IamEnvironmentKey, IdempotencyKey,
    TestingEnvironmentCreate, TestingEnvironmentIamPairing, TestingEnvironmentUpdate,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

async fn management(app: &App, headers: &HeaderMap) -> Result<Client> {
    let client = session::client(app, headers).await?;
    if client.config().environment().is_some() {
        return Err(bad(
            "Manage testing environments from your production organisation session.",
        ));
    }
    Ok(client)
}
fn json(value: impl serde::Serialize) -> Result<Json<Value>> {
    Ok(Json(
        serde_json::to_value(value).map_err(|_| bad("Unexpected environment response"))?,
    ))
}
fn key(operation: Uuid) -> Result<IdempotencyKey> {
    if operation.is_nil() {
        return Err(bad("Missing operation identity"));
    }
    Ok(IdempotencyKey::new(operation.to_string())?)
}

#[derive(Deserialize)]
pub(crate) struct Listing {
    status: Option<String>,
}
pub(crate) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<Listing>,
) -> Result<Json<Value>> {
    if input
        .status
        .as_deref()
        .is_some_and(|status| !matches!(status, "active" | "deleted"))
    {
        return Err(bad("Choose active or deleted environments."));
    }
    json(
        management(&app, &headers)
            .await?
            .testing_environments(input.status.as_deref())
            .await?,
    )
}

// Do not derive Debug or Serialize: these request-only inputs contain IAM secrets.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pairing {
    iam_environment_id: Uuid,
    iam_environment_key: String,
    iam_app_id: String,
    iam_app_secret: String,
}
impl Pairing {
    fn into_sdk(self) -> Result<TestingEnvironmentIamPairing> {
        Ok(TestingEnvironmentIamPairing::new(
            self.iam_environment_id,
            IamEnvironmentKey::new(self.iam_environment_key)?,
            ApplicationId::new(self.iam_app_id)?,
            IamApplicationSecret::new(self.iam_app_secret)?,
        ))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Create {
    name: String,
    description: Option<String>,
    iam_test_key: Option<String>,
    operation_id: Uuid,
}
pub(crate) async fn create(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Create>,
) -> Result<Json<Value>> {
    let operation = key(input.operation_id)?;
    let client = management(&app, &headers).await?;
    let mut create = TestingEnvironmentCreate::new(input.name);
    create.iam_test_key = input.iam_test_key.map(IamEnvironmentKey::new).transpose()?;
    create.description = input.description;
    json(
        client
            .create_testing_environment_with_key(&create, &operation)
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Edit {
    version: i64,
    name: String,
    description: Option<String>,
    operation_id: Uuid,
}
pub(crate) async fn edit(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Edit>,
) -> Result<Json<Value>> {
    let operation = key(input.operation_id)?;
    if input.version < 1 {
        return Err(bad("Invalid environment version"));
    }
    json(
        management(&app, &headers)
            .await?
            .update_testing_environment_with_key(
                id,
                input.version,
                &TestingEnvironmentUpdate {
                    name: Some(input.name),
                    description: Some(input.description),
                },
                &operation,
            )
            .await?,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RePair {
    pairing: Pairing,
    operation_id: Uuid,
}
pub(crate) async fn pair(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RePair>,
) -> Result<Json<Value>> {
    let operation = key(input.operation_id)?;
    let client = management(&app, &headers).await?;
    json(
        client
            .replace_testing_environment_iam_pairing_with_key(
                id,
                &input.pairing.into_sdk()?,
                &operation,
            )
            .await?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    operation_id: Uuid,
}
pub(crate) async fn action(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, action)): Path<(Uuid, String)>,
    Json(input): Json<Intent>,
) -> Result<Json<Value>> {
    let operation = key(input.operation_id)?;
    let client = management(&app, &headers).await?;
    match action.as_str() {
        "retire" => json(
            client
                .delete_testing_environment_with_key(id, &operation)
                .await?,
        ),
        "restore" => json(
            client
                .restore_testing_environment_with_key(id, &operation)
                .await?,
        ),

        "clean" => json(
            client
                .clean_testing_environment_with_key(id, &operation)
                .await?,
        ),
        _ => Err(bad("Unknown environment action")),
    }
}
// Browser POST requires CSRF protection and explicit action. The SDK performs
// the backend's audited GET and enforces creator/administrator authority.
pub(crate) async fn reveal(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>> {
    json(
        management(&app, &headers)
            .await?
            .testing_environment_key(id)
            .await?,
    )
}
