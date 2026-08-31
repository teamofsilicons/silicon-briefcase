//! `UUIDv7` identifiers owned by Briefcase.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// An invalid Briefcase-owned identifier.
#[derive(Debug, Error)]
pub enum DomainIdError {
    /// The textual value is not a UUID.
    #[error("invalid UUID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    /// Briefcase identifiers are generated as `UUIDv7` values.
    #[error("Briefcase identifiers must be UUIDv7")]
    NotVersionSeven,
}

fn validate_uuid_v7(value: Uuid) -> Result<Uuid, DomainIdError> {
    if value.get_version_num() == 7 {
        Ok(value)
    } else {
        Err(DomainIdError::NotVersionSeven)
    }
}

macro_rules! domain_id {
    ($(#[$metadata:meta])* $name:ident) => {
        $(#[$metadata])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a time-ordered `UUIDv7` identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Validates and wraps an existing `UUIDv7` identifier.
            ///
            /// # Errors
            ///
            /// Returns [`DomainIdError`] unless `value` is version seven.
            pub fn from_uuid(value: Uuid) -> Result<Self, DomainIdError> {
                validate_uuid_v7(value).map(Self)
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = DomainIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_uuid(Uuid::parse_str(value)?)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = Uuid::deserialize(deserializer)?;
                Self::from_uuid(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

domain_id!(
    /// A file or folder identifier.
    EntryId
);
domain_id!(
    /// An explicit permission-grant identifier.
    GrantId
);
domain_id!(
    /// A multipart-upload session identifier.
    MultipartUploadId
);
domain_id!(
    /// A retained file-version identifier.
    VersionId
);
domain_id!(
    /// An access-request identifier.
    AccessRequestId
);
domain_id!(
    /// An organization storage-configuration identifier.
    StorageConfigurationId
);

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{DomainIdError, EntryId};

    #[test]
    fn generated_identifiers_are_uuid_v7() {
        assert_eq!(EntryId::new().as_uuid().get_version_num(), 7);
    }

    #[test]
    fn non_v7_identifiers_are_rejected() {
        assert!(matches!(
            EntryId::from_uuid(Uuid::new_v4()),
            Err(DomainIdError::NotVersionSeven)
        ));
    }
}
