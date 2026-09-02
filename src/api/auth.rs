//! Per-operation online IAM authentication for HTTP handlers.

use http::{HeaderMap, HeaderName, header::AUTHORIZATION};
use secrecy::SecretString;

use crate::{
    domain::actor::{ApplicationId, OrganizationId, RequestAuthContext},
    error::AppError,
    infrastructure::iam::IamClient,
    request_context,
};

const ORG_ID: HeaderName = HeaderName::from_static("x-org-id");
const OBO_PROOF: HeaderName = HeaderName::from_static("x-iam-obo-access-proof");
const APP_ID: HeaderName = HeaderName::from_static("x-app-id");
const MAX_CREDENTIAL_BYTES: usize = 8_192;

/// Stable IAM action bound to one Briefcase operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IamAction {
    /// List visible children.
    ListEntries,
    /// Create a folder.
    CreateFolder,
    /// Read entry metadata.
    ReadEntry,
    /// Rename or move an entry.
    UpdateEntry,
    /// Move an entry to the bin.
    DeleteEntry,
    /// Stream current file content for rendering.
    ReadContent,
    /// Download current file content.
    DownloadFile,
    /// Upload a small file.
    UploadFile,
    /// Initialize multipart upload.
    InitiateMultipart,
    /// Upload one multipart part.
    UploadMultipartPart,
    /// Complete multipart upload.
    CompleteMultipart,
    /// Abort multipart upload.
    AbortMultipart,
    /// List explicit permissions.
    ListPermissions,
    /// Inspect the caller's own effective access on named targets.
    InspectPermissions,
    /// Grant explicit permission.
    GrantPermission,
    /// Revoke explicit permission.
    RevokePermission,
    /// Request entry access.
    CreateAccessRequest,
    /// Decide an access request.
    DecideAccessRequest,
    /// Search visible content.
    Search,
    /// Read the notification inbox.
    ListNotifications,
    /// Mark the notification inbox read.
    ReadNotifications,
    /// Read an entry's action history.
    ListActivity,
    /// List retained versions.
    ListVersions,
    /// Restore a retained version.
    RestoreVersion,
    /// List recoverable bin entries.
    ListBin,
    /// Restore a recoverable entry.
    RestoreBinEntry,
    /// Configure organization storage.
    ConfigureStorage,
}

impl IamAction {
    /// Returns the action string recorded in IAM OBO capabilities.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ListEntries => "briefcase.entries.list",
            Self::CreateFolder => "briefcase.folder.create",
            Self::ReadEntry => "briefcase.entry.read",
            Self::UpdateEntry => "briefcase.entry.update",
            Self::DeleteEntry => "briefcase.entry.delete",
            Self::ReadContent => "briefcase.file.read_content",
            Self::DownloadFile => "briefcase.file.download",
            Self::UploadFile => "briefcase.file.upload",
            Self::InitiateMultipart => "briefcase.multipart.initiate",
            Self::UploadMultipartPart => "briefcase.multipart.upload_part",
            Self::CompleteMultipart => "briefcase.multipart.complete",
            Self::AbortMultipart => "briefcase.multipart.abort",
            Self::ListPermissions => "briefcase.permissions.list",
            Self::InspectPermissions => "briefcase.permissions.inspect",
            Self::GrantPermission => "briefcase.permissions.grant",
            Self::RevokePermission => "briefcase.permissions.revoke",
            Self::CreateAccessRequest => "briefcase.access_request.create",
            Self::DecideAccessRequest => "briefcase.access_request.decide",
            Self::Search => "briefcase.search",
            Self::ListNotifications => "briefcase.notifications.list",
            Self::ReadNotifications => "briefcase.notifications.read",
            Self::ListActivity => "briefcase.activity.list",
            Self::ListVersions => "briefcase.versions.list",
            Self::RestoreVersion => "briefcase.versions.restore",
            Self::ListBin => "briefcase.bin.list",
            Self::RestoreBinEntry => "briefcase.bin.restore",
            Self::ConfigureStorage => "briefcase.storage.configure",
        }
    }
}

/// IAM-verified identity plus request correlation passed to application services.
#[derive(Clone, Debug)]
pub struct AuthenticatedRequest {
    authorization: RequestAuthContext,
    request_id: String,
}

impl AuthenticatedRequest {
    /// Returns the trusted authorization facts.
    #[must_use]
    pub const fn authorization(&self) -> &RequestAuthContext {
        &self.authorization
    }

    /// Returns the correlation identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

/// Authenticates one operation with a direct IAM bearer token.
///
/// The contracted API is a bearer surface. An application acts through the one
/// exposed OBO endpoint instead, where IAM binds the proof to the exact
/// request; presenting an OBO proof anywhere else is a request error rather
/// than a partially honored credential.
///
/// # Errors
///
/// Returns an opaque public error for missing, malformed, inactive, expired,
/// mismatched, or unverifiable credentials.
pub async fn authenticate(
    iam: &IamClient,
    headers: &HeaderMap,
    action: IamAction,
    resource: &str,
) -> Result<AuthenticatedRequest, AppError> {
    let organization = parse_organization(headers)?;
    let authorization = require_bearer_only(headers)?;
    let token = parse_bearer(authorization)?;
    let verified = iam.introspect_bearer(&token, &organization).await?;
    if verified.authorization().organization_id() != &organization {
        return Err(AppError::Forbidden);
    }
    tracing::debug!(
        iam.action = action.as_str(),
        iam.resource = resource,
        "IAM bearer identity verified"
    );
    let request_id = request_context::current_request_id().ok_or(AppError::Internal {
        category: "request_scope_missing",
    })?;

    Ok(AuthenticatedRequest {
        authorization: verified.into_authorization(),
        request_id,
    })
}

/// Validates the credential shape before a streaming handler reads a body.
///
/// # Errors
///
/// Returns an opaque authentication or request error for missing, ambiguous,
/// duplicated, or syntactically invalid security headers.
pub(crate) fn require_bearer_shape(headers: &HeaderMap) -> Result<(), AppError> {
    // Parse the organization here as well so a streaming endpoint rejects a
    // malformed tenant boundary before it admits request-body bytes.
    parse_organization(headers)?;
    parse_bearer(require_bearer_only(headers)?)?;
    Ok(())
}

/// Authenticates an operation that explicitly permits only a direct bearer
/// token, such as organization storage administration.
pub async fn authenticate_bearer(
    iam: &IamClient,
    headers: &HeaderMap,
    action: IamAction,
) -> Result<AuthenticatedRequest, AppError> {
    let organization = parse_organization(headers)?;
    authenticate(iam, headers, action, organization.as_str()).await
}

/// Reads the application identity and proof presented to the OBO endpoint.
///
/// # Errors
///
/// Returns an opaque public error when either credential is absent, malformed,
/// duplicated, or accompanied by a bearer token.
pub(crate) fn obo_credentials(
    headers: &HeaderMap,
) -> Result<(ApplicationId, SecretString), AppError> {
    if optional_single_header(headers, &AUTHORIZATION)?.is_some() {
        return Err(AppError::bad_request("ambiguous_authentication"));
    }
    let proof = optional_single_header(headers, &OBO_PROOF)?.ok_or(AppError::Unauthenticated)?;
    let app_id = optional_single_header(headers, &APP_ID)?.ok_or(AppError::Unauthenticated)?;
    let application = ApplicationId::new(app_id.to_owned())
        .map_err(|_| AppError::bad_request("invalid_app_id"))?;
    Ok((application, parse_secret_header(proof)?))
}

/// Reads an optional tenant header, for a route where IAM names the tenant.
///
/// # Errors
///
/// Returns a request error when the header is duplicated or malformed.
pub(crate) fn optional_organization(
    headers: &HeaderMap,
) -> Result<Option<OrganizationId>, AppError> {
    optional_single_header(headers, &ORG_ID)?
        .map(|value| {
            OrganizationId::new(value.to_owned())
                .map_err(|_| AppError::bad_request("invalid_org_id"))
        })
        .transpose()
}

fn require_bearer_only(headers: &HeaderMap) -> Result<&str, AppError> {
    if optional_single_header(headers, &OBO_PROOF)?.is_some()
        || optional_single_header(headers, &APP_ID)?.is_some()
    {
        return Err(AppError::bad_request("ambiguous_authentication"));
    }
    optional_single_header(headers, &AUTHORIZATION)?.ok_or(AppError::Unauthenticated)
}

/// Returns the validated organization identifier used as an IAM resource for
/// organization-wide operations.
pub fn organization_resource(headers: &HeaderMap) -> Result<String, AppError> {
    Ok(parse_organization(headers)?.into_inner())
}

fn parse_organization(headers: &HeaderMap) -> Result<OrganizationId, AppError> {
    let value = required_single_header(headers, &ORG_ID, "missing_org_id")?;
    OrganizationId::new(value.to_owned()).map_err(|_| AppError::bad_request("invalid_org_id"))
}

fn parse_bearer(value: &str) -> Result<SecretString, AppError> {
    let mut parts = value.split_ascii_whitespace();
    let scheme = parts.next();
    let token = parts.next();
    if !scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer"))
        || token.is_none()
        || parts.next().is_some()
    {
        return Err(AppError::Unauthenticated);
    }
    parse_secret_header(token.unwrap_or_default())
}

fn parse_secret_header(value: &str) -> Result<SecretString, AppError> {
    if value.is_empty() || value.len() > MAX_CREDENTIAL_BYTES {
        return Err(AppError::Unauthenticated);
    }
    Ok(SecretString::from(value.to_owned()))
}

fn required_single_header<'a>(
    headers: &'a HeaderMap,
    name: &HeaderName,
    missing_code: &'static str,
) -> Result<&'a str, AppError> {
    optional_single_header(headers, name)?.ok_or_else(|| AppError::bad_request(missing_code))
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
                .map_err(|_| AppError::bad_request("invalid_security_header"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, HeaderValue};

    use super::{IamAction, obo_credentials, parse_bearer, require_bearer_shape};

    #[test]
    fn action_names_are_stable_capabilities() {
        let expected = [
            (IamAction::ListEntries, "briefcase.entries.list"),
            (IamAction::CreateFolder, "briefcase.folder.create"),
            (IamAction::ReadEntry, "briefcase.entry.read"),
            (IamAction::UpdateEntry, "briefcase.entry.update"),
            (IamAction::DeleteEntry, "briefcase.entry.delete"),
            (IamAction::ReadContent, "briefcase.file.read_content"),
            (IamAction::DownloadFile, "briefcase.file.download"),
            (IamAction::UploadFile, "briefcase.file.upload"),
            (IamAction::InitiateMultipart, "briefcase.multipart.initiate"),
            (
                IamAction::UploadMultipartPart,
                "briefcase.multipart.upload_part",
            ),
            (IamAction::CompleteMultipart, "briefcase.multipart.complete"),
            (IamAction::AbortMultipart, "briefcase.multipart.abort"),
            (IamAction::ListPermissions, "briefcase.permissions.list"),
            (
                IamAction::InspectPermissions,
                "briefcase.permissions.inspect",
            ),
            (IamAction::GrantPermission, "briefcase.permissions.grant"),
            (IamAction::RevokePermission, "briefcase.permissions.revoke"),
            (
                IamAction::CreateAccessRequest,
                "briefcase.access_request.create",
            ),
            (
                IamAction::DecideAccessRequest,
                "briefcase.access_request.decide",
            ),
            (IamAction::Search, "briefcase.search"),
            (IamAction::ListNotifications, "briefcase.notifications.list"),
            (IamAction::ReadNotifications, "briefcase.notifications.read"),
            (IamAction::ListActivity, "briefcase.activity.list"),
            (IamAction::ListVersions, "briefcase.versions.list"),
            (IamAction::RestoreVersion, "briefcase.versions.restore"),
            (IamAction::ListBin, "briefcase.bin.list"),
            (IamAction::RestoreBinEntry, "briefcase.bin.restore"),
            (IamAction::ConfigureStorage, "briefcase.storage.configure"),
        ];

        for (action, name) in expected {
            assert_eq!(action.as_str(), name);
        }
    }

    #[test]
    fn the_bearer_surface_accepts_only_a_bearer_token() {
        let mut bearer = HeaderMap::new();
        bearer.insert("x-org-id", HeaderValue::from_static("org_example"));
        bearer.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer opaque-token"),
        );
        assert!(require_bearer_shape(&bearer).is_ok());

        let mut proof_only = HeaderMap::new();
        proof_only.insert("x-org-id", HeaderValue::from_static("org_example"));
        proof_only.insert(
            "x-iam-obo-access-proof",
            HeaderValue::from_static("opaque-proof"),
        );
        proof_only.insert("x-app-id", HeaderValue::from_static("app_example"));
        // An application must use the OBO endpoint, not the bearer surface.
        assert!(require_bearer_shape(&proof_only).is_err());
        assert!(obo_credentials(&proof_only).is_ok());
    }

    #[test]
    fn ambiguous_credential_shape_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-org-id", HeaderValue::from_static("org_example"));
        headers.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer opaque-token"),
        );
        headers.insert(
            "x-iam-obo-access-proof",
            HeaderValue::from_static("opaque-proof"),
        );
        headers.insert("x-app-id", HeaderValue::from_static("app_example"));

        assert!(require_bearer_shape(&headers).is_err());
        assert!(obo_credentials(&headers).is_err());
    }

    #[test]
    fn bearer_parser_rejects_ambiguous_values() {
        assert!(parse_bearer("Bearer one two").is_err());
        assert!(parse_bearer("Basic token").is_err());
        assert!(parse_bearer("bearer token").is_ok());
    }
}
