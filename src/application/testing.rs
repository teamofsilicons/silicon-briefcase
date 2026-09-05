//! Briefcase testing-environment lifecycle models.

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Maximum number of simultaneously active sandboxes in one deployment.
pub const MAX_ACTIVE_TESTING_ENVIRONMENTS: i64 = 10;
/// Per-environment storage ceiling, in binary gigabytes.
pub const TESTING_ENVIRONMENT_STORAGE_LIMIT_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// Lifecycle state visible through the control plane.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TestingEnvironmentStatus {
    /// The root key selects the isolated data plane.
    Active,
    /// The key is destroyed and data remains recoverable until `purge_after`.
    Deleted,
}

/// Creation request accepted on the production control plane.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestingEnvironmentCreate {
    /// Human-readable name unique among the organization's active sandboxes.
    pub name: String,
    /// Optional purpose or CI-run description.
    pub description: Option<String>,
    /// Public UUID of the separately created IAM testing environment.
    pub iam_environment_id: Uuid,
    /// IAM environment root key used only on outbound IAM calls.
    pub iam_environment_key: SecretString,
    /// Canonical test-only Briefcase Application ID.
    pub iam_app_id: String,
    /// Test-only Briefcase Application secret returned by IAM.
    pub iam_app_secret: SecretString,
}

impl std::fmt::Debug for TestingEnvironmentCreate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TestingEnvironmentCreate")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("iam_environment_id", &self.iam_environment_id)
            .field("iam_environment_key", &"<redacted>")
            .field("iam_app_id", &self.iam_app_id)
            .field("iam_app_secret", &"<redacted>")
            .finish()
    }
}

/// Replacement IAM testing-plane pairing for an existing environment.
///
/// Re-pairing preserves the Briefcase root key and isolated data while
/// replacing every IAM credential as one atomic unit.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestingEnvironmentIamPairing {
    /// Public UUID of the replacement IAM testing environment.
    pub iam_environment_id: Uuid,
    /// Replacement IAM testing-environment root key.
    pub iam_environment_key: SecretString,
    /// Canonical Briefcase Application ID, equal to the configured service ID.
    pub iam_app_id: String,
    /// Fresh test-only Briefcase Application secret returned by IAM.
    pub iam_app_secret: SecretString,
}

impl std::fmt::Debug for TestingEnvironmentIamPairing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TestingEnvironmentIamPairing")
            .field("iam_environment_id", &self.iam_environment_id)
            .field("iam_environment_key", &"<redacted>")
            .field("iam_app_id", &self.iam_app_id)
            .field("iam_app_secret", &"<redacted>")
            .finish()
    }
}

/// Mutable non-secret metadata.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestingEnvironmentPatch {
    /// Replacement name, when present.
    pub name: Option<String>,
    /// Replacement description. An explicit `null` clears it.
    pub description: Option<Option<String>>,
}

/// Public lifecycle representation; contains no credential material.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironment {
    /// Public stable UUID.
    pub id: Uuid,
    /// Production organization that owns this environment.
    pub org_id: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Current lifecycle state.
    pub status: TestingEnvironmentStatus,
    /// IAM environment paired with this Briefcase environment.
    pub iam_environment_id: Uuid,
    /// IAM Application used only inside that IAM environment.
    pub iam_app_id: String,
    /// Public actor that created the environment.
    pub created_by: TestingEnvironmentCreator,
    /// Monotonic Briefcase-root-key generation.
    pub key_generation: i64,
    /// Last root-key rotation.
    #[serde(with = "time::serde::rfc3339::option")]
    pub key_rotated_at: Option<OffsetDateTime>,
    /// Last accepted data-plane request.
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
    /// Most recent clean operation.
    #[serde(with = "time::serde::rfc3339::option")]
    pub cleaned_at: Option<OffsetDateTime>,
    /// Soft-deletion time.
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
    /// Permanent-destruction deadline.
    #[serde(with = "time::serde::rfc3339::option")]
    pub purge_after: Option<OffsetDateTime>,
    /// Optimistic-concurrency version.
    pub version: i64,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last control-plane update.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Creator identity retained independently of later membership state.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironmentCreator {
    /// `carbon` or `silicon`.
    #[serde(rename = "type")]
    pub actor_type: String,
    /// Public IAM actor handle.
    pub id: String,
}

/// Lifecycle response that reveals the generated Briefcase root key.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironmentWithKey {
    /// Non-secret environment state.
    #[serde(flatten)]
    pub environment: TestingEnvironment,
    /// Exact 32-character alphanumeric Briefcase root key.
    pub key: String,
}

/// Audited root-key retrieval response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironmentKey {
    /// Environment whose key was returned.
    pub environment_id: Uuid,
    /// Current generation.
    pub key_generation: i64,
    /// Last rotation timestamp.
    #[serde(with = "time::serde::rfc3339::option")]
    pub key_rotated_at: Option<OffsetDateTime>,
    /// Exact root key.
    pub key: String,
}

/// Key-authorized self-description, intentionally minimal.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironmentSelf {
    /// Public environment UUID.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Root-key generation.
    pub key_generation: i64,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Result of erasing one environment's Briefcase state.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TestingEnvironmentCleaning {
    /// Environment that was reset.
    pub environment_id: Uuid,
    /// Database rows logically removed after provider cleanup was durably queued.
    pub erased_rows: u64,
    /// Completion timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub cleaned_at: OffsetDateTime,
}

/// Cursor-free page; only its active subset is deployment-bounded at ten.
#[derive(Clone, Debug, Serialize)]
pub struct TestingEnvironmentPage {
    /// Environments visible to the organization.
    pub items: Vec<TestingEnvironment>,
}
