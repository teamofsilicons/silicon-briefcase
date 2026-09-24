//! Bounded lifetimes for expiring shares and self-destructing files.
//!
//! Both features take a time between one minute and one month, set in whole
//! minutes. A month is 30 days (UNDERSTANDING.md §Expiring, §Self Destruct).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// A lifetime in whole minutes, from 1 minute to 30 days inclusive.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct LifetimeMinutes(u32);

/// The requested lifetime is outside 1 minute to 30 days.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("a lifetime must be between 1 and 43200 minutes (1 minute to 30 days)")]
pub struct LifetimeOutOfRange;

impl LifetimeMinutes {
    /// Shortest lifetime: one minute.
    pub const MIN: u32 = 1;
    /// Longest lifetime: 30 days.
    pub const MAX: u32 = 30 * 24 * 60;

    /// Validates a lifetime.
    ///
    /// # Errors
    ///
    /// Returns [`LifetimeOutOfRange`] outside 1 to 43,200 minutes.
    pub const fn new(minutes: u32) -> Result<Self, LifetimeOutOfRange> {
        if minutes < Self::MIN || minutes > Self::MAX {
            return Err(LifetimeOutOfRange);
        }
        Ok(Self(minutes))
    }

    /// Returns the lifetime in minutes.
    #[must_use]
    pub const fn minutes(self) -> u32 {
        self.0
    }

    /// Returns the lifetime in minutes as PostgreSQL's `integer`.
    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
    pub const fn as_i32(self) -> i32 {
        // MAX (43,200) is far below i32::MAX.
        self.0 as i32
    }
}

impl<'de> Deserialize<'de> for LifetimeMinutes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let minutes = u32::deserialize(deserializer)?;
        Self::new(minutes).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for LifetimeMinutes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} minutes", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_one_minute_to_thirty_days() {
        assert_eq!(LifetimeMinutes::new(0), Err(LifetimeOutOfRange));
        assert_eq!(LifetimeMinutes::new(1).map(LifetimeMinutes::minutes), Ok(1));
        assert_eq!(
            LifetimeMinutes::new(43_200).map(LifetimeMinutes::minutes),
            Ok(43_200)
        );
        assert_eq!(LifetimeMinutes::new(43_201), Err(LifetimeOutOfRange));
    }

    #[test]
    fn deserialization_enforces_bounds() {
        assert!(serde_json::from_str::<LifetimeMinutes>("60").is_ok());
        assert!(serde_json::from_str::<LifetimeMinutes>("0").is_err());
        assert!(serde_json::from_str::<LifetimeMinutes>("43201").is_err());
        assert!(serde_json::from_str::<LifetimeMinutes>("1.5").is_err());
    }
}
