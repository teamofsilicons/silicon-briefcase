//! File-version numbering, provenance, and retention policy.

use std::{fmt, num::NonZeroU64};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::ids::VersionId;

/// Maximum number of current and historical versions retained per file.
pub const MAX_RETAINED_VERSIONS: usize = 50;

/// A monotonically increasing, one-based file-version number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct VersionNumber(NonZeroU64);

impl VersionNumber {
    /// The first version produced by an initial upload.
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    /// Validates a persisted version number.
    ///
    /// # Errors
    ///
    /// Returns [`VersionNumberError::Zero`] when `value` is zero.
    pub const fn new(value: u64) -> Result<Self, VersionNumberError> {
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(VersionNumberError::Zero),
        }
    }

    /// Returns the integer representation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Calculates the next monotonically increasing number.
    ///
    /// # Errors
    ///
    /// Returns [`VersionNumberError::Overflow`] when the next number cannot be
    /// represented by `u64`.
    pub fn checked_next(self) -> Result<Self, VersionNumberError> {
        self.get()
            .checked_add(1)
            .ok_or(VersionNumberError::Overflow)
            .and_then(Self::new)
    }
}

impl fmt::Display for VersionNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for VersionNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Invalid version-number data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum VersionNumberError {
    /// Version numbers are one-based.
    #[error("version number must start at one")]
    Zero,
    /// The next version cannot be represented by `u64`.
    #[error("version number overflowed")]
    Overflow,
}

/// Why a retained version was created in v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "source")]
pub enum VersionSource {
    /// The file's initial small or multipart upload.
    InitialUpload,
    /// A retained version copied into a new current version.
    Restore {
        /// Historical version whose bytes were copied.
        source_version_id: VersionId,
    },
}

#[cfg(test)]
mod tests {
    use super::{VersionNumber, VersionNumberError};

    #[test]
    fn version_numbers_are_one_based_and_checked() {
        assert_eq!(VersionNumber::new(0), Err(VersionNumberError::Zero));
        assert_eq!(
            VersionNumber::FIRST.checked_next().map(VersionNumber::get),
            Ok(2)
        );
        assert_eq!(
            VersionNumber::new(u64::MAX).and_then(VersionNumber::checked_next),
            Err(VersionNumberError::Overflow)
        );
    }
}
