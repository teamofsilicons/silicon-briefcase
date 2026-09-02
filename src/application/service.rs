//! Transactional metadata application services.
//!
//! This module owns use-case orchestration and keeps HTTP, `SQLx`, and object
//! storage details behind explicit ports.

mod access_requests;
mod applications;
mod bin;
mod entries;
mod error;
mod model;
mod notifications;
mod permissions;
mod repository;
mod search;
mod versions;

use std::sync::Arc;

use crate::{
    application::context::ExecutionContext,
    domain::permission::{Capability, EffectiveAuthorization, EntryVisibility},
};

pub use error::{MetadataServiceError, ValidationError};
pub use model::*;
pub use repository::{MetadataRepository, MetadataRepositoryError};

/// Reusable application service for Briefcase's contracted metadata domains.
#[derive(Clone)]
pub struct MetadataService {
    pub(super) repository: Arc<dyn MetadataRepository>,
}

impl MetadataService {
    /// Constructs the service from its metadata repository port.
    #[must_use]
    pub fn new(repository: Arc<dyn MetadataRepository>) -> Self {
        Self { repository }
    }
}

pub(super) fn validate_context(context: &ExecutionContext) -> Result<(), ValidationError> {
    if context.request_id().trim().is_empty() {
        Err(ValidationError {
            field: "request_id",
            message: "must not be empty",
        })
    } else {
        Ok(())
    }
}

pub(super) fn require_capability(
    entry: &AuthorizableEntry,
    context: &ExecutionContext,
    required: Capability,
) -> Result<EffectiveAuthorization, MetadataServiceError> {
    let authorization = entry.authorization(context.authorization());
    if authorization.visibility() != EntryVisibility::Full {
        return Err(MetadataServiceError::NotFound);
    }
    if !authorization.allows(required) {
        return Err(MetadataServiceError::Forbidden { required });
    }
    Ok(authorization)
}
