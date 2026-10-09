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
use secrecy::{ExposeSecret as _, SecretString};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    application::{
        context::ExecutionContext,
        idempotency::{IdempotencyKey, bytes_fingerprint},
        service::MutationMetadata,
    },
    domain::{
        account::Actor,
        actor::{
            ActorId, ActorKind, ActorRef, AuthenticationMode, OrganizationRole, RequestAuthContext,
        },
        entry::EntryPath,
        ids::{EntryId, GrantId, VersionId},
    },
    error::AppError,
    request_context,
};

use super::{
    auth::{self, Action},
    state::AppState,
    upload::StageUploadError,
    validation::ValidationErrors,
};

const IDEMPOTENCY_KEY: HeaderName = HeaderName::from_static("idempotency-key");

/// Authenticates a request carrying the caller's own Silicon Accounts access
/// token. `resource` names what is acted on, for logs only.
pub(crate) async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    action: Action,
    resource: &str,
) -> Result<ExecutionContext, AppError> {
    authenticate_with_token(state, headers, action, resource)
        .await
        .map(|(context, _)| context)
}

/// Like [`authenticate`], also returning the verified token for calls made
/// back to Silicon Accounts with it (such as reading the account's profile).
pub(crate) async fn authenticate_with_token(
    state: &AppState,
    headers: &HeaderMap,
    action: Action,
    resource: &str,
) -> Result<(ExecutionContext, SecretString), AppError> {
    let token = auth::bearer_token(headers)?;
    let actor = state
        .accounts
        .authenticate_token(token.expose_secret(), action.requires_introspection())
        .await?;
    tracing::debug!(
        accounts.action = action.as_str(),
        accounts.resource = resource,
        accounts.introspected = action.requires_introspection(),
        "Silicon Accounts access token verified"
    );
    Ok((execution_context(actor)?, token))
}

/// Authenticates an app acting for an account with a User verification proof
/// that must carry `scope`.
pub(crate) async fn authenticate_proof(
    state: &AppState,
    headers: &HeaderMap,
    scope: &'static str,
) -> Result<ExecutionContext, AppError> {
    let token = auth::proof_token(headers)?;
    let actor = state
        .accounts
        .authenticate_proof(token.expose_secret(), scope)
        .await?;
    tracing::debug!(
        accounts.scope = scope,
        accounts.issuing_app = actor.issuing_app().unwrap_or_default(),
        "Silicon Accounts proof verified"
    );
    execution_context(actor)
}

fn execution_context(actor: Actor) -> Result<ExecutionContext, AppError> {
    let request_id = request_context::current_request_id().ok_or(AppError::Internal {
        category: "request_scope_missing",
    })?;
    let authorization = RequestAuthContext::for_account(actor).map_err(|_| AppError::Internal {
        category: "account_context",
    })?;
    Ok(ExecutionContext::new(authorization, request_id))
}

/// Resolves a share recipient the caller named by `c:`/`si:` id (or uuid) to
/// the account it is now, keyed by its uuid.
pub(crate) async fn resolve_recipient(
    state: &AppState,
    kind: ActorKind,
    id: &str,
) -> Result<ActorRef, AppError> {
    let record = state.accounts.resolve(id).await?;
    if record.kind != kind {
        let (actual, expected) = (kind_title(record.kind), kind_title(kind));
        return Err(AppError::unprocessable(
            "principal_kind_mismatch",
            format!(
                "{id} is a {actual}, not a {expected}; name it with type {}.",
                record_kind_name(record.kind)
            ),
        ));
    }
    let id = ActorId::new(record.uuid).map_err(|_| AppError::Internal {
        category: "account_uuid",
    })?;
    Ok(ActorRef::new(record.kind, id))
}

/// Attaches resolved recipients so the transaction can name them in grants.
/// They never become the caller.
pub(crate) fn with_recipients(
    context: ExecutionContext,
    recipients: &[ActorRef],
) -> ExecutionContext {
    if recipients.is_empty() {
        return context;
    }
    let tenant = context.authorization().organization_id().clone();
    let members = recipients
        .iter()
        .map(|recipient| {
            RequestAuthContext::new(
                tenant.clone(),
                recipient.clone(),
                OrganizationRole::Member,
                Vec::new(),
                AuthenticationMode::Bearer,
            )
        })
        .collect();
    context.with_directory_members(members)
}

const fn kind_title(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "Carbon",
        ActorKind::Silicon => "Silicon",
    }
}

const fn record_kind_name(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "carbon",
        ActorKind::Silicon => "silicon",
    }
}

pub(crate) fn scoped<T>(
    context: &ExecutionContext,
    future: impl Future<Output = T>,
) -> impl Future<Output = T> {
    crate::telemetry::authenticated(
        context,
        request_context::scope_authenticated(
            context.request_id().to_owned(),
            context.authorization().clone(),
            Box::pin(future),
        ),
    )
}

/// The caller's tenant key, used in permanent URLs and request fingerprints.
pub(crate) fn tenant(context: &ExecutionContext) -> String {
    context
        .authorization()
        .organization_id()
        .as_str()
        .to_owned()
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

/// Validates a permanent-URL location against the caller's tenant.
///
/// A different tenant or an unparsable path is reported as not found: the
/// permanent URL must never confirm that an inaccessible entry exists.
pub(crate) fn entry_location(
    (organization, path): (String, String),
    context: &ExecutionContext,
) -> Result<(String, EntryPath), AppError> {
    if tenant(context) != organization {
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
