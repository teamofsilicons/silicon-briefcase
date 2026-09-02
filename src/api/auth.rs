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

/// Credential shape presented by a request before online IAM verification.
///
/// This is intentionally not an authenticated identity. It exists so
/// streaming handlers can verify bearer credentials before reading a body and
/// defer an OBO proof only until its exact resource binding is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialMode {
    /// A direct IAM bearer token.
    Bearer,
    /// An IAM OBO proof paired with its originating application.
    OnBehalfOf,
}

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
            Self::GrantPermission => "briefcase.permissions.grant",
            Self::RevokePermission => "briefcase.permissions.revoke",
            Self::CreateAccessRequest => "briefcase.access_request.create",
            Self::DecideAccessRequest => "briefcase.access_request.decide",
            Self::Search => "briefcase.search",
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

/// Authenticates one operation using exactly one supported IAM credential mode.
///
/// Bearer and OBO credentials are mutually exclusive. OBO verification binds
/// the proof to the route's exact action and resource before Briefcase policy
/// is evaluated by the application service.
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
    let bearer = optional_single_header(headers, &AUTHORIZATION)?;
    let proof = optional_single_header(headers, &OBO_PROOF)?;
    let app_id = optional_single_header(headers, &APP_ID)?;

    let verified = match (bearer, proof, app_id) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(AppError::bad_request("ambiguous_authentication"));
        }
        (Some(authorization), None, None) => {
            let token = parse_bearer(authorization)?;
            iam.introspect_bearer(&token, &organization).await?
        }
        (None, Some(proof), Some(app_id)) => {
            let proof = parse_secret_header(proof)?;
            let application = ApplicationId::new(app_id.to_owned())
                .map_err(|_| AppError::bad_request("invalid_app_id"))?;
            iam.verify_obo(
                &proof,
                &application,
                &organization,
                action.as_str(),
                Some(resource),
            )
            .await?
        }
        (None, None | Some(_), None) | (None, None, Some(_)) => {
            return Err(AppError::Unauthenticated);
        }
    };
    if verified.authorization().organization_id() != &organization {
        return Err(AppError::Forbidden);
    }
    let request_id = request_context::current_request_id().ok_or(AppError::Internal {
        category: "request_scope_missing",
    })?;

    Ok(AuthenticatedRequest {
        authorization: verified.into_authorization(),
        request_id,
    })
}

/// Validates the mutually exclusive authentication header shape without
/// consuming an OBO proof.
///
/// # Errors
///
/// Returns an opaque authentication or request error for missing, ambiguous,
/// duplicated, or syntactically invalid security headers.
pub(crate) fn credential_mode(headers: &HeaderMap) -> Result<CredentialMode, AppError> {
    // Parse the organization here as well so a streaming endpoint rejects a
    // malformed tenant boundary before it admits request-body bytes.
    parse_organization(headers)?;
    let bearer = optional_single_header(headers, &AUTHORIZATION)?;
    let proof = optional_single_header(headers, &OBO_PROOF)?;
    let app_id = optional_single_header(headers, &APP_ID)?;

    match (bearer, proof, app_id) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            Err(AppError::bad_request("ambiguous_authentication"))
        }
        (Some(authorization), None, None) => {
            parse_bearer(authorization)?;
            Ok(CredentialMode::Bearer)
        }
        (None, Some(proof), Some(app_id)) => {
            parse_secret_header(proof)?;
            ApplicationId::new(app_id.to_owned())
                .map_err(|_| AppError::bad_request("invalid_app_id"))?;
            Ok(CredentialMode::OnBehalfOf)
        }
        (None, None | Some(_), None) | (None, None, Some(_)) => Err(AppError::Unauthenticated),
    }
}

/// Authenticates an operation that explicitly permits only a direct bearer
/// token, such as organization storage administration.
pub async fn authenticate_bearer(
    iam: &IamClient,
    headers: &HeaderMap,
    action: IamAction,
) -> Result<AuthenticatedRequest, AppError> {
    let organization = parse_organization(headers)?;
    let authorization =
        optional_single_header(headers, &AUTHORIZATION)?.ok_or(AppError::Unauthenticated)?;
    if optional_single_header(headers, &OBO_PROOF)?.is_some()
        || optional_single_header(headers, &APP_ID)?.is_some()
    {
        return Err(AppError::bad_request("ambiguous_authentication"));
    }
    let token = parse_bearer(authorization)?;
    let verified = iam.introspect_bearer(&token, &organization).await?;
    if verified.authorization().organization_id() != &organization {
        return Err(AppError::Forbidden);
    }
    tracing::debug!(
        iam.action = action.as_str(),
        iam.resource = organization.as_str(),
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

    use super::{CredentialMode, IamAction, credential_mode, parse_bearer};

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
    fn credential_shape_is_validated_before_streaming() {
        let mut bearer = HeaderMap::new();
        bearer.insert("x-org-id", HeaderValue::from_static("org_example"));
        bearer.insert(
            http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer opaque-token"),
        );
        assert!(matches!(
            credential_mode(&bearer),
            Ok(CredentialMode::Bearer)
        ));

        let mut obo = HeaderMap::new();
        obo.insert("x-org-id", HeaderValue::from_static("org_example"));
        obo.insert(
            "x-iam-obo-access-proof",
            HeaderValue::from_static("opaque-proof"),
        );
        obo.insert("x-app-id", HeaderValue::from_static("app_example"));
        assert!(matches!(
            credential_mode(&obo),
            Ok(CredentialMode::OnBehalfOf)
        ));
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

        assert!(credential_mode(&headers).is_err());
    }

    #[test]
    fn bearer_parser_rejects_ambiguous_values() {
        assert!(parse_bearer("Bearer one two").is_err());
        assert!(parse_bearer("Basic token").is_err());
        assert!(parse_bearer("bearer token").is_ok());
    }
}
