//! Explicit extractor and request-metadata normalization.

use std::future::Future;

use axum::{
    Json,
    extract::{
        Multipart, Path, Query,
        multipart::{MultipartError, MultipartRejection},
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
};
use http::{HeaderMap, HeaderName, StatusCode};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    application::{
        context::{ExecutionContext, TestingEnvironmentContext},
        idempotency::{IdempotencyKey, bytes_fingerprint},
        service::MutationMetadata,
    },
    domain::{
        entry::EntryPath,
        ids::{AccessRequestId, EntryId, GrantId, VersionId},
    },
    error::AppError,
    infrastructure::{
        iam::IamEnvironmentCredential,
        testing::{TestingEnvironmentAccess, TestingEnvironmentUseFence},
    },
    request_context,
};

use super::{
    auth::{self, IamAction},
    state::AppState,
    upload::StageUploadError,
    validation::ValidationErrors,
};

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

pub(crate) async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    action: IamAction,
    resource: &str,
) -> Result<ExecutionContext, AppError> {
    let access = optional_testing_access(state, headers).await?;
    if access.as_ref().is_some_and(|access| {
        auth::organization_resource(headers).ok().as_deref() != Some(&access.owner_org_id)
    }) {
        return Err(AppError::NotFound);
    }
    let credential = access
        .as_ref()
        .map(iam_environment_credential)
        .transpose()?;
    let testing_environment = access.as_ref().map(|access| {
        TestingEnvironmentContext::new(access.environment_id, access.control_version)
    });
    let authenticated = auth::authenticate(
        &state.iam,
        &state.metadata,
        headers,
        action,
        resource,
        credential.as_ref(),
        testing_environment,
    )
    .await?;
    touch_testing_access(state, access.as_ref()).await?;
    Ok(execution_context(&authenticated, testing_environment))
}

pub(crate) async fn authenticate_bearer(
    state: &AppState,
    headers: &HeaderMap,
    action: IamAction,
) -> Result<ExecutionContext, AppError> {
    let access = optional_testing_access(state, headers).await?;
    if access.as_ref().is_some_and(|access| {
        auth::organization_resource(headers).ok().as_deref() != Some(&access.owner_org_id)
    }) {
        return Err(AppError::NotFound);
    }
    let credential = access
        .as_ref()
        .map(iam_environment_credential)
        .transpose()?;
    let testing_environment = access.as_ref().map(|access| {
        TestingEnvironmentContext::new(access.environment_id, access.control_version)
    });
    let authenticated = auth::authenticate_bearer(
        &state.iam,
        &state.metadata,
        headers,
        action,
        credential.as_ref(),
        testing_environment,
    )
    .await?;
    touch_testing_access(state, access.as_ref()).await?;
    Ok(execution_context(&authenticated, testing_environment))
}

/// Authenticates lifecycle management strictly on the production plane.
pub(crate) async fn production_authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<ExecutionContext, AppError> {
    if auth::testing_environment_key(headers)?.is_some() {
        return Err(AppError::bad_request("production_control_plane_required"));
    }
    let authenticated = auth::authenticate_bearer(
        &state.iam,
        &state.metadata,
        headers,
        IamAction::ManageTestingEnvironments,
        None,
        None,
    )
    .await?;
    Ok(ExecutionContext::new(
        authenticated.authorization().clone(),
        authenticated.request_id(),
    ))
}

/// Resolves the required test-plane root key without treating it as actor auth.
pub(crate) async fn testing_access(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<TestingEnvironmentAccess, AppError> {
    optional_testing_access(state, headers)
        .await?
        .ok_or_else(|| AppError::bad_request("testing_environment_required"))
}

/// Resolves an optional sandbox selector for ordinary/auth-broker routes.
pub(crate) async fn optional_testing_access(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<TestingEnvironmentAccess>, AppError> {
    let Some(key) = auth::testing_environment_key(headers)? else {
        return Ok(None);
    };
    let store = state
        .testing
        .as_ref()
        .ok_or(AppError::DependencyUnavailable {
            dependency: "testing_database",
        })?;
    store.resolve_root_key(&key).await.map(Some)
}

pub(crate) fn iam_environment_credential(
    access: &TestingEnvironmentAccess,
) -> Result<IamEnvironmentCredential, AppError> {
    IamEnvironmentCredential::new(
        access.iam_environment_key.clone(),
        access.iam_app_id.clone(),
        access.iam_app_secret.clone(),
    )
    .map(|credential| credential.with_environment_id(access.iam_environment_id))
    .map_err(|_| AppError::Internal {
        category: "testing_environment_iam_credential",
    })
}

pub(crate) async fn touch_testing_access(
    state: &AppState,
    access: Option<&TestingEnvironmentAccess>,
) -> Result<(), AppError> {
    if let Some(access) = access {
        state
            .testing
            .as_ref()
            .ok_or(AppError::DependencyUnavailable {
                dependency: "testing_database",
            })?
            .touch(access)
            .await?;
    }
    Ok(())
}

/// Acquires a shared lifecycle fence and revalidates the exact root generation.
pub(crate) async fn testing_use_fence(
    state: &AppState,
    access: Option<&TestingEnvironmentAccess>,
) -> Result<Option<TestingEnvironmentUseFence>, AppError> {
    let Some(access) = access else {
        return Ok(None);
    };
    let fence = state
        .testing
        .as_ref()
        .ok_or(AppError::DependencyUnavailable {
            dependency: "testing_database",
        })?
        .acquire_use_fence(access)
        .await?;
    Ok(Some(fence))
}

fn execution_context(
    authenticated: &auth::AuthenticatedRequest,
    testing_environment: Option<TestingEnvironmentContext>,
) -> ExecutionContext {
    testing_environment.map_or_else(
        || {
            ExecutionContext::new(
                authenticated.authorization().clone(),
                authenticated.request_id(),
            )
        },
        |testing_environment| {
            ExecutionContext::in_testing_environment(
                authenticated.authorization().clone(),
                authenticated.request_id(),
                testing_environment,
            )
        },
    )
}

pub(crate) async fn scoped<T>(context: &ExecutionContext, future: impl Future<Output = T>) -> T {
    request_context::scope_authenticated(
        context.request_id().to_owned(),
        context.authorization().clone(),
        future,
    )
    .await
}

pub(crate) fn organization_resource(headers: &HeaderMap) -> Result<String, AppError> {
    auth::organization_resource(headers)
}

pub(crate) fn json<T>(value: Result<Json<T>, JsonRejection>) -> Result<T, AppError> {
    value.map(|Json(value)| value).map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge
        } else {
            AppError::bad_request("invalid_json")
        }
    })
}

pub(crate) fn query<T>(value: Result<Query<T>, QueryRejection>) -> Result<T, AppError> {
    value
        .map(|Query(value)| value)
        .map_err(|_| AppError::bad_request("invalid_query"))
}

pub(crate) fn path<T>(value: Result<Path<T>, PathRejection>) -> Result<T, AppError> {
    value
        .map(|Path(value)| value)
        .map_err(|_| AppError::bad_request("invalid_path_parameter"))
}

pub(crate) fn multipart(
    value: Result<Multipart, MultipartRejection>,
) -> Result<Multipart, AppError> {
    value.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge
        } else {
            AppError::bad_request("invalid_multipart")
        }
    })
}

pub(crate) fn validation(result: Result<(), ValidationErrors>) -> Result<(), AppError> {
    result.map_err(|_| AppError::validation("invalid_request"))
}

pub(crate) fn mutation<T>(
    headers: &HeaderMap,
    operation: &'static str,
    resource: &str,
    payload: &T,
    key_required: bool,
) -> Result<MutationMetadata, AppError>
where
    T: Serialize + ?Sized,
{
    let idempotency_key = idempotency_key(headers)?;
    if key_required && idempotency_key.is_none() {
        return Err(AppError::bad_request("missing_idempotency_key"));
    }
    let request_fingerprint = request_fingerprint(operation, resource, payload)?;
    Ok(MutationMetadata::new(idempotency_key, request_fingerprint))
}

pub(crate) fn idempotency_key(headers: &HeaderMap) -> Result<Option<IdempotencyKey>, AppError> {
    optional_single_header(headers, &IDEMPOTENCY_KEY)?
        .map(|value| {
            IdempotencyKey::new(value.to_owned())
                .map_err(|_| AppError::bad_request("invalid_idempotency_key"))
        })
        .transpose()
}

pub(crate) fn required_idempotency_key(headers: &HeaderMap) -> Result<IdempotencyKey, AppError> {
    idempotency_key(headers)?.ok_or_else(|| AppError::bad_request("missing_idempotency_key"))
}

pub(crate) fn request_fingerprint<T>(
    operation: &'static str,
    resource: &str,
    payload: &T,
) -> Result<[u8; 32], AppError>
where
    T: Serialize + ?Sized,
{
    let canonical = serde_json::to_vec(&(resource, payload)).map_err(|_| AppError::Internal {
        category: "request_fingerprint",
    })?;
    Ok(bytes_fingerprint(operation, &canonical))
}

/// Validates a permanent-URL location against the authenticated tenant header.
///
/// A mismatched organization or an unparsable path is reported as not found:
/// the permanent URL must never confirm that an inaccessible entry exists.
pub(crate) fn entry_location(
    (organization, path): (String, String),
    headers: &HeaderMap,
) -> Result<(String, EntryPath), AppError> {
    if auth::organization_resource(headers)? != organization {
        return Err(AppError::NotFound);
    }
    let path = EntryPath::new(path).map_err(|_| AppError::NotFound)?;
    Ok((organization, path))
}

pub(crate) fn entry_id(value: Uuid) -> Result<EntryId, AppError> {
    EntryId::from_uuid(value).map_err(|_| AppError::NotFound)
}

pub(crate) fn grant_id(value: Uuid) -> Result<GrantId, AppError> {
    GrantId::from_uuid(value).map_err(|_| AppError::NotFound)
}

pub(crate) fn access_request_id(value: Uuid) -> Result<AccessRequestId, AppError> {
    AccessRequestId::from_uuid(value).map_err(|_| AppError::NotFound)
}

pub(crate) fn version_id(value: &str) -> Result<VersionId, AppError> {
    value
        .parse()
        .map_err(|_| AppError::bad_request("invalid_version_id"))
}

pub(crate) fn map_staging_error(error: StageUploadError) -> AppError {
    match error {
        StageUploadError::TooLarge => AppError::PayloadTooLarge,
        StageUploadError::Multipart(error) => map_multipart_error(&error),
        StageUploadError::Body(_) => AppError::bad_request("invalid_upload_body"),
        StageUploadError::Io(_) => AppError::Internal {
            category: "temporary_upload_storage",
        },
    }
}

pub(crate) fn map_multipart_error(error: &MultipartError) -> AppError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::PayloadTooLarge
    } else {
        AppError::bad_request("invalid_multipart")
    }
}

fn optional_single_header<'a>(
    headers: &'a HeaderMap,
    name: &HeaderName,
) -> Result<Option<&'a str>, AppError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(AppError::bad_request("duplicate_security_header"));
    }
    first
        .map(|value| {
            value
                .to_str()
                .map_err(|_| AppError::bad_request("invalid_request_header"))
        })
        .transpose()
}
