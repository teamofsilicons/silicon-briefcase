//! Transport-level validation that complements domain constructors.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::domain::filter::MAX_FILTER_LENGTH;

use super::dto::{
    AccessDecisionDto, AccessRequestCreateDto, AccessRequestDecisionDto, ActorTypeDto,
    BucketConfigurationDto, EncryptionModeDto, EntryPatchDto, FolderCreateDto, GrantAccessDto,
    ListEntriesQuery, PageQuery, PermissionGrantCreateDto, PermissionInspectionDto, RootTypeDto,
    SearchQueryDto,
};

const MAXIMUM_CURSOR_LENGTH: usize = 2_048;
const MAXIMUM_SEARCH_LENGTH: usize = 512;
const MAXIMUM_INVITEES: usize = 100;
const MAXIMUM_INSPECTED_TARGETS: usize = 100;
const MAXIMUM_EXTERNAL_IDENTIFIER_BYTES: usize = 255;
const MAXIMUM_ARN_BYTES: usize = 2_048;

/// Stable field-oriented validation details returned to the API error layer.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ValidationErrors {
    fields: BTreeMap<String, Vec<String>>,
}

impl ValidationErrors {
    /// Adds a stable validation message for a field.
    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.fields
            .entry(field.into())
            .or_default()
            .push(message.into());
    }

    /// Returns whether no issues were recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Converts details into the JSON shape used by `AppError::Validation`.
    #[must_use]
    pub fn into_json(self) -> Value {
        serde_json::json!({ "fields": self.fields })
    }

    fn finish(self) -> Result<(), Self> {
        if self.is_empty() { Ok(()) } else { Err(self) }
    }
}

/// Validates entry-list pagination.
///
/// # Errors
///
/// Returns all invalid pagination fields.
pub fn list_entries(query: &ListEntriesQuery) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    if query.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
        errors.push("limit", "must be between 1 and 100");
    }
    if query.parent_id.is_some() && query.path.is_some() {
        errors.push("path", "must not be combined with parent_id");
    }
    if query
        .path
        .as_ref()
        .is_some_and(|path| path.trim().is_empty())
    {
        errors.push("path", "must not be blank when provided");
    }
    if let Some(filter) = query.filter.as_deref() {
        if filter.trim().is_empty() {
            errors.push("filter", "must not be blank when provided");
        }
        if filter.len() > MAX_FILTER_LENGTH {
            errors.push("filter", "is too long");
        }
    }
    if query
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.len() > MAXIMUM_CURSOR_LENGTH)
    {
        errors.push("cursor", "is too long");
    }
    errors.finish()
}

/// Validates cursor pagination for a simple listing.
///
/// # Errors
///
/// Returns all invalid pagination fields.
pub fn page(query: &PageQuery) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    if query.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
        errors.push("limit", "must be between 1 and 100");
    }
    if query
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.len() > MAXIMUM_CURSOR_LENGTH)
    {
        errors.push("cursor", "is too long");
    }
    errors.finish()
}

/// Validates a folder creation request before domain conversion.
///
/// # Errors
///
/// Returns all invalid folder fields and invitees.
pub fn create_folder(request: &FolderCreateDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    validate_name(&request.name, "name", &mut errors);

    if request.parent_id.is_some() && request.parent_path.is_some() {
        errors.push("parent_path", "must not be combined with parent_id");
    }
    if request
        .parent_path
        .as_ref()
        .is_some_and(|path| path.trim().is_empty())
    {
        errors.push("parent_path", "must not be blank when provided");
    }
    let parent = request
        .parent_id
        .map(|_| ())
        .or_else(|| request.parent_path.as_ref().map(|_| ()));
    match (parent, request.root_type, request.tag.as_deref()) {
        (None, None, _) => errors.push("root_type", "is required when no parent is named"),
        (Some(()), Some(_), _) => {
            errors.push("root_type", "must be omitted when creating below a parent");
        }
        (Some(()), None, Some(_)) => {
            errors.push("tag", "must be omitted when creating below a parent");
        }
        (_, Some(RootTypeDto::Tag), None) => {
            errors.push("tag", "is required for a tag root");
        }
        (_, Some(RootTypeDto::Tag), Some(tag)) if !valid_external_identifier(tag) => {
            errors.push("tag", "must be a valid IAM tag identifier");
        }
        (_, Some(RootTypeDto::Public | RootTypeDto::Private), Some(_)) => {
            errors.push("tag", "is allowed only for a tag root");
        }
        _ => {}
    }

    if request.invitees.len() > MAXIMUM_INVITEES {
        errors.push("invitees", "must contain at most 100 grants");
    }
    let mut principals = BTreeSet::new();
    for (index, invitee) in request.invitees.iter().enumerate() {
        validate_grant(invitee, &format!("invitees[{index}]"), &mut errors);
        let identity = (invitee.principal.actor_type, invitee.principal.id.as_str());
        if !principals.insert(identity) {
            errors.push(
                format!("invitees[{index}].principal"),
                "must not appear more than once",
            );
        }
    }
    errors.finish()
}

/// Validates a rename/move patch.
///
/// # Errors
///
/// Returns all invalid patch fields.
pub fn patch_entry(request: &EntryPatchDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    if request.name.is_none() && request.parent_id.is_none() {
        errors.push("body", "must contain name, parent_id, or both");
    }
    if let Some(name) = request.name.as_deref() {
        validate_name(name, "name", &mut errors);
    }
    errors.finish()
}

/// Validates a permission grant request.
///
/// # Errors
///
/// Returns all invalid permission fields.
pub fn grant_permission(request: &PermissionGrantCreateDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    validate_grant(request, "grant", &mut errors);
    errors.finish()
}

/// Validates a batch permission inspection.
///
/// # Errors
///
/// Returns all invalid inspection fields.
pub fn inspect_permissions(request: &PermissionInspectionDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    let total = request.entry_ids.len().saturating_add(request.paths.len());
    if total == 0 {
        errors.push("targets", "must name at least one entry or path");
    }
    if total > MAXIMUM_INSPECTED_TARGETS {
        errors.push("targets", "must name at most 100 entries and paths");
    }
    if request.paths.iter().any(|path| path.trim().is_empty()) {
        errors.push("paths", "must not contain a blank path");
    }
    errors.finish()
}

/// Validates an access request.
///
/// # Errors
///
/// Returns all invalid access-request fields.
pub fn request_access(request: &AccessRequestCreateDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    validate_access_rights(&request.access, "access", &mut errors);
    if let Some(reason) = request.reason.as_deref() {
        if reason.trim().is_empty() {
            errors.push("reason", "must not be blank when provided");
        }
        if reason.chars().count() > 1_000 {
            errors.push("reason", "must contain at most 1000 characters");
        }
    }
    errors.finish()
}

/// Validates the conditional decision payload.
///
/// # Errors
///
/// Returns all inconsistent access-decision fields.
pub fn decide_access(request: &AccessRequestDecisionDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    match (request.decision, request.access.as_deref()) {
        (AccessDecisionDto::Approve, None) => {
            errors.push("access", "is required when approving");
        }
        (AccessDecisionDto::Approve, Some(access)) => {
            validate_access_rights(access, "access", &mut errors);
        }
        (AccessDecisionDto::Deny, Some(_)) => {
            errors.push("access", "must be omitted when denying");
        }
        (AccessDecisionDto::Deny, None) => {}
    }
    errors.finish()
}

/// Validates a search request.
///
/// # Errors
///
/// Returns all invalid search fields.
pub fn search(query: &SearchQueryDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    let normalized = query.q.trim();
    if normalized.is_empty() {
        errors.push("q", "must not be blank");
    } else if normalized.chars().count() > MAXIMUM_SEARCH_LENGTH {
        errors.push("q", "must contain at most 512 characters");
    }
    if query.limit.is_some_and(|limit| !(1..=20).contains(&limit)) {
        errors.push("limit", "must be between 1 and 20");
    }
    errors.finish()
}

/// Validates an organization-owned bucket configuration.
///
/// # Errors
///
/// Returns all invalid bucket, role, prefix, account, and encryption fields.
pub fn bucket_configuration(request: &BucketConfigurationDto) -> Result<(), ValidationErrors> {
    let mut errors = ValidationErrors::default();
    if !valid_bucket_name(&request.bucket_name) {
        errors.push(
            "bucket_name",
            "must be a valid DNS-compatible S3 bucket name",
        );
    }
    if !valid_region(&request.region) {
        errors.push("region", "must be a valid AWS region identifier");
    }
    if !valid_account_id(&request.aws_account_id) {
        errors.push("aws_account_id", "must contain exactly 12 digits");
    }
    let expected_role_prefix = format!("arn:aws:iam::{}:role/", request.aws_account_id);
    let role_name = request.role_arn.strip_prefix(&expected_role_prefix);
    if request.role_arn.len() > MAXIMUM_ARN_BYTES || !role_name.is_some_and(valid_iam_role_path) {
        errors.push(
            "role_arn",
            "must be an IAM role ARN in the configured AWS account",
        );
    }
    if request.prefix.is_empty()
        || request.prefix.trim() != request.prefix
        || request.prefix.starts_with('/')
        || request.prefix.ends_with('/')
        || request.prefix.contains("//")
        || request.prefix.contains('\\')
        || request.prefix.chars().any(char::is_control)
        || request
            .prefix
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
        || request.prefix.len() > 512
    {
        errors.push("prefix", "must be a safe non-empty relative S3 prefix");
    }
    match (request.encryption_mode, request.kms_key_arn.as_deref()) {
        (EncryptionModeDto::SseKms, None | Some("")) => {
            errors.push("kms_key_arn", "is required for sse_kms");
        }
        (EncryptionModeDto::SseKms, Some(key_arn))
            if !valid_kms_key_arn(key_arn, &request.region, &request.aws_account_id) =>
        {
            errors.push(
                "kms_key_arn",
                "must be a KMS key ARN in the configured account and region",
            );
        }
        (EncryptionModeDto::SseS3, Some(_)) => {
            errors.push("kms_key_arn", "must be omitted for sse_s3");
        }
        _ => {}
    }
    errors.finish()
}

fn validate_grant(request: &PermissionGrantCreateDto, prefix: &str, errors: &mut ValidationErrors) {
    if !valid_external_identifier(&request.principal.id) {
        errors.push(
            format!("{prefix}.principal.id"),
            "must be a valid IAM identifier of 1 to 255 bytes",
        );
    }
    if request.principal.actor_type == ActorTypeDto::Application {
        errors.push(
            format!("{prefix}.principal.type"),
            "must be carbon or silicon",
        );
    }
    validate_access_rights(&request.access, &format!("{prefix}.access"), errors);
}

fn validate_access_rights(access: &[GrantAccessDto], field: &str, errors: &mut ValidationErrors) {
    if access.is_empty() {
        errors.push(field.to_owned(), "must not be empty");
    }
    let unique = access.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != access.len() {
        errors.push(field.to_owned(), "must not contain duplicate rights");
    }
}

fn validate_name(value: &str, field: &str, errors: &mut ValidationErrors) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(field, "must not be blank");
    } else if trimmed.len() > 255 {
        errors.push(field, "must contain at most 255 bytes");
    }
    if matches!(trimmed, "." | "..") || trimmed.contains(['/', '\0']) {
        errors.push(field, "contains a reserved path value");
    }
}

fn valid_bucket_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
        && !value.contains("..")
        && !value.contains(".-")
        && !value.contains("-.")
        && value.parse::<std::net::Ipv4Addr>().is_err()
}

fn valid_region(value: &str) -> bool {
    (3..=32).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value.contains('-')
}

fn valid_account_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_external_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= MAXIMUM_EXTERNAL_IDENTIFIER_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_iam_role_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'+' | b'=' | b',' | b'.' | b'@' | b'_' | b'-')
        })
}

fn valid_kms_key_arn(value: &str, region: &str, account_id: &str) -> bool {
    let prefix = format!("arn:aws:kms:{region}:{account_id}:key/");
    value.len() <= MAXIMUM_ARN_BYTES
        && value.strip_prefix(&prefix).is_some_and(|key_id| {
            !key_id.is_empty()
                && key_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::{bucket_configuration, create_folder, decide_access};
    use crate::api::dto::{
        AccessDecisionDto, AccessRequestDecisionDto, BucketConfigurationDto, EncryptionModeDto,
        FolderCreateDto, RootTypeDto,
    };

    #[test]
    fn tag_root_requires_a_tag() {
        let result = create_folder(&FolderCreateDto {
            parent_path: None,
            name: "Engineering".to_owned(),
            parent_id: None,
            root_type: Some(RootTypeDto::Tag),
            tag: None,
            invitees: Vec::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn inherited_folder_rejects_an_ignored_tag() {
        let result = create_folder(&FolderCreateDto {
            parent_path: None,
            name: "Engineering".to_owned(),
            parent_id: Some(uuid::Uuid::now_v7()),
            root_type: None,
            tag: Some("engineering".to_owned()),
            invitees: Vec::new(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn approval_requires_an_access_level() {
        let result = decide_access(&AccessRequestDecisionDto {
            decision: AccessDecisionDto::Approve,
            access: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn kms_configuration_requires_a_key() {
        let result = bucket_configuration(&BucketConfigurationDto {
            bucket_name: "example-briefcase".to_owned(),
            region: "ap-south-1".to_owned(),
            role_arn: "arn:aws:iam::123456789012:role/briefcase".to_owned(),
            prefix: "organizations/tos".to_owned(),
            aws_account_id: "123456789012".to_owned(),
            encryption_mode: EncryptionModeDto::SseKms,
            kms_key_arn: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn bucket_prefix_must_be_a_canonical_relative_namespace() {
        let result = bucket_configuration(&BucketConfigurationDto {
            bucket_name: "example-briefcase".to_owned(),
            region: "ap-south-1".to_owned(),
            role_arn: "arn:aws:iam::123456789012:role/briefcase".to_owned(),
            prefix: "organizations/../other".to_owned(),
            aws_account_id: "123456789012".to_owned(),
            encryption_mode: EncryptionModeDto::SseS3,
            kms_key_arn: None,
        });
        assert!(result.is_err());
    }
}
