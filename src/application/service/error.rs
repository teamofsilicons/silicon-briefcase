//! Stable application-service failures.

use thiserror::Error;

use crate::domain::permission::Capability;

use super::repository::MetadataRepositoryError;

/// A validation failure in a typed application command.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid {field}: {message}")]
pub struct ValidationError {
    /// Stable field name.
    pub field: &'static str,
    /// Safe validation detail.
    pub message: &'static str,
}

impl ValidationError {
    pub(super) const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

/// Failure categories exposed by metadata use cases.
#[derive(Debug, Error)]
pub enum MetadataServiceError {
    /// Command invariants are invalid.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The resource is absent or must remain opaque to the caller.
    #[error("resource not found")]
    NotFound,
    /// A visible resource does not grant the required operation.
    #[error("operation requires {required:?}")]
    Forbidden {
        /// Missing fine-grained capability.
        required: Capability,
    },
    /// Current state conflicts with the requested mutation.
    #[error("operation conflicts with current state")]
    Conflict,
    /// Metadata persistence is unavailable or failed internally.
    #[error("metadata repository failure")]
    Repository(#[source] MetadataRepositoryError),
}

impl From<MetadataRepositoryError> for MetadataServiceError {
    fn from(error: MetadataRepositoryError) -> Self {
        match error {
            MetadataRepositoryError::NotFound => Self::NotFound,
            MetadataRepositoryError::Conflict => Self::Conflict,
            other => Self::Repository(other),
        }
    }
}
