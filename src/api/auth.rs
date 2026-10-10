//! Credential parsing for HTTP handlers.
//!
//! Every API route takes a Silicon Accounts access token issued to Briefcase:
//! `Authorization: Bearer <JWT>`. The delegated `/api/v1/obo/*` routes take a
//! User verification proof another app obtained instead:
//! `Authorization: Proof sap_…`. Each route accepts exactly one of the two.

use http::{HeaderMap, HeaderName, header::AUTHORIZATION};
use secrecy::SecretString;

use crate::error::AppError;

/// Longest access token Briefcase reads.
const MAX_ACCESS_TOKEN_BYTES: usize = 8_192;
/// Longest proof token Briefcase reads.
const MAX_PROOF_TOKEN_BYTES: usize = 512;
const PROOF_PREFIX: &str = "sap_";

/// One Briefcase operation, named for logs and for the revocation policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// List visible children.
    ListEntries,
    /// Create a folder.
    CreateFolder,
    /// Read entry metadata.
    ReadEntry,
    /// Rename or move an entry.
    UpdateEntry,
    /// Move an entry to the bin, or delete a self-destructing file.
    DeleteEntry,
    /// Keep a self-destructing file permanently.
    KeepEntry,
    /// Stream current file content for rendering.
    ReadContent,
    /// Download current file content.
    DownloadFile,
    /// Upload a file.
    UploadFile,
    /// List explicit permissions.
    ListPermissions,
    /// Inspect the caller's own effective access on named targets.
    InspectPermissions,
    /// Grant access, or change a share or the link access.
    GrantPermission,
    /// Revoke explicit access.
    RevokePermission,
    /// Search visible content.
    Search,
    /// Read the notification inbox.
    ListNotifications,
    /// Submit an explicit bug report.
    SubmitReport,
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
    /// Configure the bring-your-own S3 bucket.
    ConfigureStorage,
    /// Read consumption and limits.
    ReadUsage,
    /// Read the caller's own account.
    ReadAccount,
    /// List the drives the caller may open (its own and its Silicons').
    ListDrives,
    /// List what other accounts shared with the caller.
    ListShared,
    /// Read who may share with a Silicon.
    ReadShareAllowances,
    /// Change who may share with a Silicon.
    ManageShareAllowances,
}

impl Action {
    /// Returns the stable action name recorded in logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ListEntries => "briefcase.entries.list",
            Self::CreateFolder => "briefcase.folder.create",
            Self::ReadEntry => "briefcase.entry.read",
            Self::UpdateEntry => "briefcase.entry.update",
            Self::DeleteEntry => "briefcase.entry.delete",
            Self::KeepEntry => "briefcase.entry.keep",
            Self::ReadContent => "briefcase.file.read_content",
            Self::DownloadFile => "briefcase.file.download",
            Self::UploadFile => "briefcase.file.upload",
            Self::ListPermissions => "briefcase.permissions.list",
            Self::InspectPermissions => "briefcase.permissions.inspect",
            Self::GrantPermission => "briefcase.permissions.grant",
            Self::RevokePermission => "briefcase.permissions.revoke",
            Self::Search => "briefcase.search",
            Self::ListNotifications => "briefcase.notifications.list",
            Self::SubmitReport => "briefcase.reports.submit",
            Self::ReadNotifications => "briefcase.notifications.read",
            Self::ListActivity => "briefcase.activity.list",
            Self::ListVersions => "briefcase.versions.list",
            Self::RestoreVersion => "briefcase.versions.restore",
            Self::ListBin => "briefcase.bin.list",
            Self::RestoreBinEntry => "briefcase.bin.restore",
            Self::ConfigureStorage => "briefcase.storage.configure",
            Self::ReadUsage => "briefcase.usage.read",
            Self::ReadAccount => "briefcase.account.read",
            Self::ListDrives => "briefcase.drives.list",
            Self::ListShared => "briefcase.shared.list",
            Self::ReadShareAllowances => "briefcase.share_allowances.read",
            Self::ManageShareAllowances => "briefcase.share_allowances.manage",
        }
    }

    /// Whether the token must also be confirmed active with Silicon Accounts.
    ///
    /// Reads, lists, content and uploads verify the token locally (it lives
    /// at most 30 minutes, and webhook-fed sign-outs are checked on every
    /// request). Changing who can reach an entry, destroying data, restoring
    /// a version and configuring storage also introspect, so a revocation the
    /// webhook has not delivered yet still stops them.
    #[must_use]
    pub const fn requires_introspection(self) -> bool {
        matches!(
            self,
            Self::GrantPermission
                | Self::RevokePermission
                | Self::DeleteEntry
                | Self::KeepEntry
                | Self::RestoreVersion
                | Self::ConfigureStorage
                | Self::ManageShareAllowances
        )
    }
}

/// Returns the access token of a `Bearer` request.
///
/// # Errors
///
/// Returns 401 with the exact reason when the header is missing, duplicated,
/// uses another scheme, or does not hold a JWT.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Result<SecretString, AppError> {
    let (scheme, value) = authorization(headers)?;
    if scheme.eq_ignore_ascii_case("proof") {
        return Err(AppError::unauthorized(
            "proof_not_accepted_here",
            "This route takes the caller's own Silicon Accounts access token (Authorization: Bearer). Apps acting for an account use the /api/v1/obo/ routes with Authorization: Proof.",
        ));
    }
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AppError::unauthorized(
            "unsupported_authorization_scheme",
            "Send Authorization: Bearer <access token>, where the access token is a Silicon Accounts access token issued to Briefcase.",
        ));
    }
    if value.len() > MAX_ACCESS_TOKEN_BYTES || !is_jwt_shape(value) {
        return Err(AppError::unauthorized(
            "token_malformed",
            "The bearer token is not a Silicon Accounts access token (a JWT of three base64url parts). Send the access_token from Silicon Accounts, not a refresh token, short-lived token or proof.",
        ));
    }
    Ok(SecretString::from(value.to_owned()))
}

/// Returns the proof token of a `Proof` request.
///
/// # Errors
///
/// Returns 401 with the exact reason when the header is missing, duplicated,
/// uses another scheme, or does not hold a `sap_` proof token.
pub(crate) fn proof_token(headers: &HeaderMap) -> Result<SecretString, AppError> {
    let (scheme, value) = authorization(headers)?;
    if !scheme.eq_ignore_ascii_case("proof") {
        return Err(AppError::unauthorized(
            "proof_required",
            "This route is for apps acting for an account: send Authorization: Proof sap_… with a Silicon Accounts User verification proof whose receiving app is briefcase.",
        ));
    }
    if value.len() > MAX_PROOF_TOKEN_BYTES
        || !value.starts_with(PROOF_PREFIX)
        || value.len() == PROOF_PREFIX.len()
        || !value[PROOF_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::unauthorized(
            "proof_malformed",
            "The proof is not a Silicon Accounts proof token (sap_…). Send the proof_token, not the proof_refresh_token (sapr_…).",
        ));
    }
    Ok(SecretString::from(value.to_owned()))
}

/// Validates the credential shape before a streaming handler reads a body.
///
/// # Errors
///
/// Returns the [`bearer_token`] refusal.
pub(crate) fn require_bearer_shape(headers: &HeaderMap) -> Result<(), AppError> {
    bearer_token(headers).map(drop)
}

fn authorization(headers: &HeaderMap) -> Result<(&str, &str), AppError> {
    let value = optional_single_header(headers, &AUTHORIZATION)?.ok_or_else(|| {
        AppError::unauthorized(
            "authentication_required",
            "Send Authorization: Bearer <access token> with a Silicon Accounts access token issued to Briefcase.",
        )
    })?;
    let mut parts = value.split_ascii_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(scheme), Some(credential), None) => Ok((scheme, credential)),
        _ => Err(AppError::unauthorized(
            "authorization_malformed",
            "The Authorization header must be exactly `<scheme> <credential>`, such as `Bearer eyJ…`.",
        )),
    }
}

fn is_jwt_shape(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

pub(super) fn optional_single_header<'a>(
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
    use http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
    use secrecy::ExposeSecret as _;

    use super::{Action, bearer_token, proof_token, require_bearer_shape};

    const JWT: &str = "eyJhbGciOiJFZERTQSJ9.eyJzdWIiOiJ6UW8ifQ.c2lnbmF0dXJl";

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(AUTHORIZATION, value);
        }
        headers
    }

    fn code(result: Result<impl Sized, crate::error::AppError>) -> String {
        match result {
            Ok(_) => "ok".to_owned(),
            Err(crate::error::AppError::Explained { code, .. }) => code.into_owned(),
            Err(other) => format!("{other:?}"),
        }
    }

    #[test]
    fn action_names_are_stable() {
        let expected = [
            (Action::ListEntries, "briefcase.entries.list"),
            (Action::CreateFolder, "briefcase.folder.create"),
            (Action::ReadEntry, "briefcase.entry.read"),
            (Action::UpdateEntry, "briefcase.entry.update"),
            (Action::DeleteEntry, "briefcase.entry.delete"),
            (Action::KeepEntry, "briefcase.entry.keep"),
            (Action::ReadContent, "briefcase.file.read_content"),
            (Action::DownloadFile, "briefcase.file.download"),
            (Action::UploadFile, "briefcase.file.upload"),
            (Action::ListPermissions, "briefcase.permissions.list"),
            (Action::InspectPermissions, "briefcase.permissions.inspect"),
            (Action::GrantPermission, "briefcase.permissions.grant"),
            (Action::RevokePermission, "briefcase.permissions.revoke"),
            (Action::Search, "briefcase.search"),
            (Action::ListNotifications, "briefcase.notifications.list"),
            (Action::ReadNotifications, "briefcase.notifications.read"),
            (Action::ListActivity, "briefcase.activity.list"),
            (Action::ListVersions, "briefcase.versions.list"),
            (Action::RestoreVersion, "briefcase.versions.restore"),
            (Action::ListBin, "briefcase.bin.list"),
            (Action::RestoreBinEntry, "briefcase.bin.restore"),
            (Action::ConfigureStorage, "briefcase.storage.configure"),
            (Action::ReadAccount, "briefcase.account.read"),
        ];
        for (action, name) in expected {
            assert_eq!(action.as_str(), name);
        }
    }

    #[test]
    fn only_sensitive_actions_introspect() {
        for sensitive in [
            Action::GrantPermission,
            Action::RevokePermission,
            Action::DeleteEntry,
            Action::KeepEntry,
            Action::RestoreVersion,
            Action::ConfigureStorage,
        ] {
            assert!(sensitive.requires_introspection(), "{sensitive:?}");
        }
        for local in [
            Action::ListEntries,
            Action::ReadEntry,
            Action::ReadContent,
            Action::DownloadFile,
            Action::UploadFile,
            Action::Search,
            Action::RestoreBinEntry,
            Action::ReadAccount,
        ] {
            assert!(!local.requires_introspection(), "{local:?}");
        }
    }

    #[test]
    fn bearer_routes_take_exactly_one_jwt() {
        assert_eq!(
            bearer_token(&headers(&format!("Bearer {JWT}")))
                .map(|token| token.expose_secret().to_owned())
                .ok()
                .as_deref(),
            Some(JWT)
        );
        assert!(require_bearer_shape(&headers(&format!("bearer {JWT}"))).is_ok());
        assert_eq!(
            code(bearer_token(&HeaderMap::new())),
            "authentication_required"
        );
        assert_eq!(
            code(bearer_token(&headers("Bearer one two"))),
            "authorization_malformed"
        );
        assert_eq!(
            code(bearer_token(&headers("Basic dXNlcjpwdw=="))),
            "unsupported_authorization_scheme"
        );
        assert_eq!(
            code(bearer_token(&headers("Bearer sar_refresh"))),
            "token_malformed"
        );
        assert_eq!(
            code(bearer_token(&headers("Bearer oat_aaaaaaaa"))),
            "token_malformed"
        );
        assert_eq!(
            code(bearer_token(&headers("Proof sap_abc"))),
            "proof_not_accepted_here"
        );
        let mut duplicated = headers(&format!("Bearer {JWT}"));
        duplicated.append(AUTHORIZATION, HeaderValue::from_static("Bearer x.y.z"));
        assert!(matches!(
            bearer_token(&duplicated),
            Err(crate::error::AppError::BadRequest { .. })
        ));
    }

    #[test]
    fn delegated_routes_take_exactly_one_proof() {
        assert_eq!(
            proof_token(&headers(
                "Proof sap_OMGtGwcBe5QgGJng3SIp0yGOh1nxefxCufefPXqr7dk"
            ))
            .map(|token| token.expose_secret().len())
            .ok(),
            Some(47)
        );
        assert_eq!(
            code(proof_token(&headers(&format!("Bearer {JWT}")))),
            "proof_required"
        );
        assert_eq!(
            code(proof_token(&headers("Proof sapr_refresh"))),
            "proof_malformed"
        );
        assert_eq!(code(proof_token(&headers("Proof sap_"))), "proof_malformed");
        assert_eq!(
            code(proof_token(&headers("Proof sap_has space"))),
            "authorization_malformed"
        );
        assert_eq!(
            code(proof_token(&HeaderMap::new())),
            "authentication_required"
        );
    }
}
