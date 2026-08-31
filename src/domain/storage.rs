//! Organization storage-location, encryption, and validation state.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which class of S3-compatible location stores a file version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageLocationKind {
    /// Briefcase's configured platform bucket and organization prefix.
    PlatformManaged,
    /// An organization bucket accessed through an assumed IAM role.
    OrganizationManaged,
}

/// Server-side encryption required for an organization-managed location.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptionMode {
    /// S3-managed encryption keys.
    SseS3,
    /// A customer-selected AWS KMS key.
    SseKms,
}

impl EncryptionMode {
    /// Returns whether configuration must include a KMS key ARN.
    #[must_use]
    pub const fn requires_kms_key(self) -> bool {
        matches!(self, Self::SseKms)
    }
}

/// Durable activation state of a versioned storage configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageConfigurationState {
    /// Awaiting account verification and create/read/overwrite/delete probing.
    PendingValidation,
    /// Validation and cleanup succeeded; new versions may use the location.
    Active,
    /// Validation failed; the previous active location remains selected.
    Failed,
}

impl StorageConfigurationState {
    /// Applies the only valid transition for a newly proposed configuration.
    ///
    /// # Errors
    ///
    /// Returns [`StorageConfigurationTransitionError`] if this configuration
    /// has already reached a terminal validation state.
    pub const fn finish_validation(
        self,
        outcome: StorageValidationOutcome,
    ) -> Result<Self, StorageConfigurationTransitionError> {
        if !matches!(self, Self::PendingValidation) {
            return Err(StorageConfigurationTransitionError::AlreadyValidated { state: self });
        }
        match outcome {
            StorageValidationOutcome::Succeeded => Ok(Self::Active),
            StorageValidationOutcome::Failed => Ok(Self::Failed),
        }
    }
}

/// Redaction-safe result of destructive-safe storage validation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StorageValidationOutcome {
    /// Account checks, object operations, and final cleanup all succeeded.
    Succeeded,
    /// At least one validation or cleanup requirement failed.
    Failed,
}

/// Invalid storage-configuration state transition.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StorageConfigurationTransitionError {
    /// Versioned storage configurations are immutable after validation.
    #[error("storage configuration has already reached {state:?}")]
    AlreadyValidated {
        /// Existing terminal state.
        state: StorageConfigurationState,
    },
}

#[cfg(test)]
mod tests {
    use super::{EncryptionMode, StorageConfigurationState, StorageValidationOutcome};

    #[test]
    fn kms_mode_requires_a_key_reference() {
        assert!(!EncryptionMode::SseS3.requires_kms_key());
        assert!(EncryptionMode::SseKms.requires_kms_key());
    }

    #[test]
    fn only_pending_configurations_can_finish_validation() {
        assert_eq!(
            StorageConfigurationState::PendingValidation
                .finish_validation(StorageValidationOutcome::Succeeded),
            Ok(StorageConfigurationState::Active)
        );
        assert!(
            StorageConfigurationState::Active
                .finish_validation(StorageValidationOutcome::Failed)
                .is_err()
        );
    }
}
