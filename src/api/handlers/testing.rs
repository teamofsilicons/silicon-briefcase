//! Briefcase testing-environment control- and data-plane handlers.

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use crate::{
    application::testing::{
        TestingEnvironmentCreate, TestingEnvironmentIamPairing, TestingEnvironmentPatch,
        TestingEnvironmentStatus,
    },
    error::AppError,
    infrastructure::{iam::IamEnvironmentCredential, testing::TestingEnvironmentStore},
    request_context,
};

use super::super::{extract, state::AppState};

/// Status selector accepted by the bounded, cursor-free environment listing.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListStatus {
    Active,
    Deleted,
    All,
}

/// Query accepted by the environment listing.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    status: Option<ListStatus>,
}

/// Merge-patch body that preserves the distinction between an absent
/// description and an explicit `null`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatchRequest {
    name: Option<String>,
    #[serde(default)]
    description: DescriptionPatch,
}

/// Three-state merge-patch value for the optional description field.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
enum DescriptionPatch {
    /// The property was absent, so its stored value is unchanged.
    #[default]
    Unchanged,
    /// The property was explicitly `null`, so its stored value is cleared.
    Clear,
    /// The property carried a string, so it replaces the stored value.
    Replace(String),
}

impl<'de> Deserialize<'de> for DescriptionPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<String>::deserialize(deserializer)? {
            Some(value) => Self::Replace(value),
            None => Self::Clear,
        })
    }
}

/// Lists the organization's Briefcase testing environments.
pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<crate::application::testing::TestingEnvironmentPage>, AppError> {
    let org_id = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let query = extract::query(query)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let status = match query.status.unwrap_or(ListStatus::Active) {
        ListStatus::Active => Some(TestingEnvironmentStatus::Active),
        ListStatus::Deleted => Some(TestingEnvironmentStatus::Deleted),
        ListStatus::All => None,
    };
    let page = testing_store(&state)?.list(&context, status).await?;
    Ok(Json(page))
}

/// Creates an empty Briefcase testing environment and returns its root key.
pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<String>, PathRejection>,
    body: Result<Json<TestingEnvironmentCreate>, JsonRejection>,
) -> Result<Response, AppError> {
    let org_id = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let body = extract::json(body)?;
    let mutation = extract::mutation(
        &headers,
        "createTestingEnvironment",
        &org_id,
        &(
            body.name.as_str(),
            body.description.as_deref(),
            body.iam_environment_id,
            body.iam_environment_key.expose_secret(),
            body.iam_app_id.as_str(),
            body.iam_app_secret.expose_secret(),
        ),
        true,
    )?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let store = testing_store(&state)?;
    if let Some(replayed) = store.replay_create(&context, &mutation).await? {
        return response_with_etag(
            StatusCode::CREATED,
            replayed.environment.version,
            replayed,
            true,
        );
    }
    let iam_environment = IamEnvironmentCredential::new(
        body.iam_environment_key.clone(),
        body.iam_app_id.clone(),
        body.iam_app_secret.clone(),
    )
    .map_err(|_| AppError::validation("invalid_testing_environment_iam_credential"))?;
    state
        .iam
        .validate_environment_credential(&iam_environment, body.iam_environment_id)
        .await?;
    let created = store.create(&context, &body, &mutation).await?;
    response_with_etag(
        StatusCode::CREATED,
        created.environment.version,
        created,
        true,
    )
}

/// Reads one testing environment without revealing any credential.
pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let environment = testing_store(&state)?.get(&context, environment_id).await?;
    response_with_etag(StatusCode::OK, environment.version, environment, false)
}

/// Atomically replaces the IAM test-plane pairing while preserving Briefcase data.
pub(crate) async fn replace_iam_pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
    body: Result<Json<TestingEnvironmentIamPairing>, JsonRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let body = extract::json(body)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(
        &headers,
        "replaceTestingEnvironmentIamPairing",
        &resource,
        &(
            body.iam_environment_id,
            body.iam_environment_key.expose_secret(),
            body.iam_app_id.as_str(),
            body.iam_app_secret.expose_secret(),
        ),
        true,
    )?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let store = testing_store(&state)?;
    if let Some(replayed) = store
        .replay_iam_pairing(&context, environment_id, &mutation)
        .await?
    {
        return response_with_etag(StatusCode::OK, replayed.version, replayed, true);
    }
    let iam_environment = IamEnvironmentCredential::new(
        body.iam_environment_key.clone(),
        body.iam_app_id.clone(),
        body.iam_app_secret.clone(),
    )
    .map_err(|_| AppError::validation("invalid_testing_environment_iam_credential"))?;
    state
        .iam
        .validate_environment_credential(&iam_environment, body.iam_environment_id)
        .await?;
    let environment = store
        .replace_iam_pairing(&context, environment_id, &body, &mutation)
        .await?;
    response_with_etag(StatusCode::OK, environment.version, environment, true)
}

/// Replaces mutable testing-environment metadata under a strong `ETag`.
pub(crate) async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
    body: Result<Json<PatchRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let body = extract::json(body)?;
    if body.name.is_none() && body.description == DescriptionPatch::Unchanged {
        return Err(AppError::validation("empty_testing_environment_patch"));
    }
    let changes = TestingEnvironmentPatch {
        name: body.name,
        description: match body.description {
            DescriptionPatch::Unchanged => None,
            DescriptionPatch::Clear => Some(None),
            DescriptionPatch::Replace(value) => Some(Some(value)),
        },
    };
    let expected_version = strong_if_match(&headers)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(
        &headers,
        "updateTestingEnvironment",
        &resource,
        &(
            expected_version,
            changes.name.as_deref(),
            changes.description.as_ref().map(|value| value.as_deref()),
        ),
        true,
    )?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let environment = testing_store(&state)?
        .update(
            &context,
            environment_id,
            expected_version,
            &changes,
            &mutation,
        )
        .await?;
    response_with_etag(StatusCode::OK, environment.version, environment, false)
}

/// Soft-deletes an environment, immediately invalidating its root key.
pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(&headers, "deleteTestingEnvironment", &resource, &(), true)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let environment = testing_store(&state)?
        .delete(&context, environment_id, &mutation)
        .await?;
    response_with_etag(StatusCode::OK, environment.version, environment, false)
}

/// Retrieves the current root key for an environment administrator.
pub(crate) async fn key(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let key = testing_store(&state)?.key(&context, environment_id).await?;
    Ok(secret_json(StatusCode::OK, key))
}

/// Rotates the root key and returns the replacement exactly once per response.
pub(crate) async fn rotate_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(
        &headers,
        "rotateTestingEnvironmentKey",
        &resource,
        &(),
        true,
    )?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let rotated = testing_store(&state)?
        .rotate_key(&context, environment_id, &mutation)
        .await?;
    response_with_etag(StatusCode::OK, rotated.environment.version, rotated, true)
}

/// Erases an environment's isolated Briefcase state through the control plane.
pub(crate) async fn clean(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(&headers, "cleanTestingEnvironment", &resource, &(), true)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let store = testing_store(&state)?;

    // The key read enforces creator/admin authority. Resolve it immediately
    // back to the secret-bearing access object required by the isolated data
    // plane; the plaintext key never enters logs or the response.
    let key = store.key(&context, environment_id).await?;
    let access = store.resolve_root_key(&SecretString::from(key.key)).await?;
    let cleaning = store
        .clean(&access, context.request_id(), &mutation)
        .await?;
    Ok(Json(cleaning).into_response())
}

/// Restores a soft-deleted environment and returns its necessarily new key.
pub(crate) async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<(String, Uuid)>, PathRejection>,
) -> Result<Response, AppError> {
    let (org_id, environment_id) = extract::path(path)?;
    require_path_organization(&headers, &org_id)?;
    let resource = format!("{org_id}/{environment_id}");
    let mutation = extract::mutation(&headers, "restoreTestingEnvironment", &resource, &(), true)?;
    let context = extract::production_authenticate(&state, &headers).await?;
    let restored = testing_store(&state)?
        .restore(&context, environment_id, &mutation)
        .await?;
    response_with_etag(StatusCode::OK, restored.environment.version, restored, true)
}

/// Describes the environment selected by the presented root key.
pub(crate) async fn current(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let access = extract::testing_access(&state, &headers).await?;
    let _fence = extract::testing_use_fence(&state, Some(&access)).await?;
    extract::touch_testing_access(&state, Some(&access)).await?;
    let current = testing_store(&state)?.current(&access);
    Ok(private_json(StatusCode::OK, current))
}

/// Erases isolated state using the environment root key as the sole authority.
pub(crate) async fn clean_current(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::application::testing::TestingEnvironmentCleaning>, AppError> {
    let access = extract::testing_access(&state, &headers).await?;
    let mutation = extract::mutation(
        &headers,
        "cleanCurrentTestingEnvironment",
        &access.environment_id.to_string(),
        &(),
        true,
    )?;
    let request_id = request_context::current_request_id().ok_or(AppError::Internal {
        category: "request_scope_missing",
    })?;
    let cleaning = testing_store(&state)?
        .clean(&access, &request_id, &mutation)
        .await?;
    Ok(Json(cleaning))
}

fn testing_store(state: &AppState) -> Result<&TestingEnvironmentStore, AppError> {
    state
        .testing
        .as_deref()
        .ok_or(AppError::DependencyUnavailable {
            dependency: "testing_database",
        })
}

fn require_path_organization(headers: &HeaderMap, path_org_id: &str) -> Result<(), AppError> {
    if extract::organization_resource(headers)? == path_org_id {
        Ok(())
    } else {
        // A cross-tenant path is deliberately indistinguishable from an
        // absent environment.
        Err(AppError::NotFound)
    }
}

fn strong_if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let mut values = headers.get_all(header::IF_MATCH).iter();
    let value = values
        .next()
        .ok_or_else(|| AppError::bad_request("missing_if_match"))?;
    if values.next().is_some() {
        return Err(AppError::bad_request("duplicate_security_header"));
    }
    let value = value
        .to_str()
        .map_err(|_| AppError::bad_request("invalid_if_match"))?;
    let digits = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::bad_request("invalid_if_match"))?;
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::bad_request("invalid_if_match"));
    }
    let version = digits
        .parse::<i64>()
        .map_err(|_| AppError::bad_request("invalid_if_match"))?;
    if version < 1 {
        return Err(AppError::bad_request("invalid_if_match"));
    }
    Ok(version)
}

fn response_with_etag<T: serde::Serialize>(
    status: StatusCode,
    version: i64,
    value: T,
    secret: bool,
) -> Result<Response, AppError> {
    let etag =
        HeaderValue::from_str(&format!("\"{version}\"")).map_err(|_| AppError::Internal {
            category: "testing_environment_etag",
        })?;
    let mut response = (status, Json(value)).into_response();
    response.headers_mut().insert(header::ETAG, etag);
    if secret {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

fn secret_json<T: serde::Serialize>(status: StatusCode, value: T) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        Json(value),
    )
        .into_response()
}

fn private_json<T: serde::Serialize>(status: StatusCode, value: T) -> Response {
    (
        status,
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        )],
        Json(value),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{DescriptionPatch, PatchRequest, private_json, strong_if_match};

    #[test]
    fn if_match_requires_one_strong_positive_integer_etag() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, HeaderValue::from_static("\"7\""));
        assert_eq!(strong_if_match(&headers).ok(), Some(7));

        for invalid in ["7", "W/\"7\"", "\"0\"", "\"-1\"", "\"\""] {
            headers.insert(
                header::IF_MATCH,
                HeaderValue::from_str(invalid)
                    .unwrap_or_else(|error| panic!("valid test header: {error}")),
            );
            assert!(strong_if_match(&headers).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn patch_deserialization_preserves_explicit_null() -> anyhow::Result<()> {
        let omitted: PatchRequest = serde_json::from_str("{}")?;
        assert_eq!(omitted.description, DescriptionPatch::Unchanged);

        let cleared: PatchRequest = serde_json::from_str(r#"{"description":null}"#)?;
        assert_eq!(cleared.description, DescriptionPatch::Clear);

        let replaced: PatchRequest = serde_json::from_str(r#"{"description":"purpose"}"#)?;
        assert_eq!(
            replaced.description,
            DescriptionPatch::Replace("purpose".to_owned())
        );
        Ok(())
    }

    #[test]
    fn root_authorized_responses_are_private_and_never_stored() {
        let response = private_json(axum::http::StatusCode::OK, serde_json::json!({"ok": true}));
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("private, no-store")
        );
    }
}
