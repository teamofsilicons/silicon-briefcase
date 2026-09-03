//! File-tree classification and entry-name invariants.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::actor::TagName;

/// Maximum UTF-8 byte length of an entry name.
pub const MAX_ENTRY_NAME_BYTES: usize = 255;

/// Whether an entry stores bytes or contains child entries.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A file with versioned object content.
    File,
    /// A folder that may contain child entries.
    Folder,
}

impl EntryKind {
    /// Returns whether this entry kind may contain children.
    #[must_use]
    pub const fn can_contain_children(self) -> bool {
        matches!(self, Self::Folder)
    }
}

/// The inherited top-level permission boundary exposed by the API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootType {
    /// Every current organization member derives read access.
    Public,
    /// Access derives from ownership or explicit grants.
    Private,
    /// Members with the matching IAM tag derive read access.
    Tag,
}

/// A complete inherited boundary, including the tag when one is required.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "root_type")]
pub enum EntryBoundary {
    /// Public organization content.
    Public,
    /// Private content.
    Private,
    /// Content readable by members with the exact current IAM tag.
    Tag {
        /// The tag inherited by every descendant.
        tag: TagName,
    },
}

impl EntryBoundary {
    /// Returns the API-facing boundary discriminator.
    #[must_use]
    pub const fn root_type(&self) -> RootType {
        match self {
            Self::Public => RootType::Public,
            Self::Private => RootType::Private,
            Self::Tag { .. } => RootType::Tag,
        }
    }

    /// Returns the tag for a tag boundary.
    #[must_use]
    pub const fn tag(&self) -> Option<&TagName> {
        match self {
            Self::Tag { tag } => Some(tag),
            Self::Public | Self::Private => None,
        }
    }
}

/// Internal classification for reconciled system folders.
///
/// System entries are never serialized as a separate v1 API kind. They are
/// reserved and cannot be renamed, moved, deleted, or granted directly.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemEntryKind {
    /// The organization's canonical Public container.
    PublicContainer,
    /// The organization's canonical Private container.
    PrivateContainer,
    /// A canonical folder for one current IAM tag.
    TagRoot,
    /// A canonical private folder for one current member.
    PrivateActorFolder,
}

impl SystemEntryKind {
    /// Returns the boundary enforced for the system entry and descendants.
    #[must_use]
    pub const fn root_type(self) -> RootType {
        match self {
            Self::PublicContainer => RootType::Public,
            Self::PrivateContainer | Self::PrivateActorFolder => RootType::Private,
            Self::TagRoot => RootType::Tag,
        }
    }
}

/// A validated, trimmed file or folder name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EntryName(String);

impl EntryName {
    /// Trims and validates a user-controlled entry name.
    ///
    /// # Errors
    ///
    /// Returns [`EntryNameError`] when the normalized name is empty, too long,
    /// contains a NUL byte or slash, or is a reserved path component.
    pub fn new(value: impl AsRef<str>) -> Result<Self, EntryNameError> {
        let trimmed = value.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EntryNameError::Empty);
        }
        if trimmed.len() > MAX_ENTRY_NAME_BYTES {
            return Err(EntryNameError::TooLong {
                actual_bytes: trimmed.len(),
                maximum_bytes: MAX_ENTRY_NAME_BYTES,
            });
        }
        if trimmed.contains('\0') {
            return Err(EntryNameError::ContainsNul);
        }
        if trimmed.contains('/') {
            return Err(EntryNameError::ContainsSlash);
        }
        if matches!(trimmed, "." | "..") {
            return Err(EntryNameError::Reserved);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the normalized name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name and returns its normalized text.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for EntryName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for EntryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EntryName {
    type Err = EntryNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for EntryName {
    type Error = EntryNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for EntryName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Maximum UTF-8 byte length of an organization-relative entry path.
pub const MAX_ENTRY_PATH_BYTES: usize = 2_048;

/// A normalized organization-relative path such as `private/cos:tos/q3.md`.
///
/// A path is the durable, human-readable address of an entry inside one
/// organization. It never carries the organization identifier, a leading or
/// trailing separator, or a traversal segment, so it can be joined into the
/// contracted permanent URL and compared byte-for-byte against persistence.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EntryPath(String);

impl EntryPath {
    /// Parses a slash-separated path whose segments are already percent-decoded.
    ///
    /// Empty leading and trailing segments are tolerated so both `a/b` and
    /// `/a/b/` address the same entry, matching the contracted URL shape.
    ///
    /// # Errors
    ///
    /// Returns [`EntryPathError`] when the path has no segments, exceeds
    /// [`MAX_ENTRY_PATH_BYTES`], contains an interior empty segment, or
    /// contains a segment that is not a valid [`EntryName`].
    pub fn new(value: impl AsRef<str>) -> Result<Self, EntryPathError> {
        let trimmed = value.as_ref().trim().trim_matches('/').trim();
        if trimmed.is_empty() {
            return Err(EntryPathError::Empty);
        }
        let mut normalized = String::with_capacity(trimmed.len());
        for segment in trimmed.split('/') {
            let name = EntryName::new(segment).map_err(EntryPathError::Segment)?;
            if !normalized.is_empty() {
                normalized.push('/');
            }
            normalized.push_str(name.as_str());
        }
        if normalized.len() > MAX_ENTRY_PATH_BYTES {
            return Err(EntryPathError::TooLong {
                actual_bytes: normalized.len(),
                maximum_bytes: MAX_ENTRY_PATH_BYTES,
            });
        }
        Ok(Self(normalized))
    }

    /// Builds the path of a direct child of this folder.
    ///
    /// # Errors
    ///
    /// Returns [`EntryPathError::TooLong`] when the child path would exceed
    /// [`MAX_ENTRY_PATH_BYTES`].
    pub fn join(&self, name: &EntryName) -> Result<Self, EntryPathError> {
        let mut child = String::with_capacity(self.0.len() + 1 + name.as_str().len());
        child.push_str(&self.0);
        child.push('/');
        child.push_str(name.as_str());
        if child.len() > MAX_ENTRY_PATH_BYTES {
            return Err(EntryPathError::TooLong {
                actual_bytes: child.len(),
                maximum_bytes: MAX_ENTRY_PATH_BYTES,
            });
        }
        Ok(Self(child))
    }

    /// Returns the normalized path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the path and returns its normalized text.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Returns the path segments in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Returns the first segment, which always names an organization root.
    #[must_use]
    pub fn root_segment(&self) -> &str {
        self.0.split('/').next().unwrap_or(&self.0)
    }

    /// Returns the last segment, which is the entry's own name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// Returns the number of segments, where an organization root has depth 1.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.0.split('/').count()
    }

    /// Returns whether the path addresses an organization root container.
    #[must_use]
    pub fn is_root(&self) -> bool {
        !self.0.contains('/')
    }

    /// Returns the parent folder path, or `None` for an organization root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| Self(parent.to_owned()))
    }
}

impl AsRef<str> for EntryPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for EntryPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EntryPath {
    type Err = EntryPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for EntryPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Why an organization-relative path is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EntryPathError {
    /// The path has no addressable segment.
    #[error("entry path cannot be empty")]
    Empty,
    /// The normalized path exceeds the persisted bound.
    #[error("entry path is {actual_bytes} bytes; maximum is {maximum_bytes}")]
    TooLong {
        /// Actual normalized UTF-8 byte length.
        actual_bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
    /// One segment is not a valid entry name.
    #[error("entry path segment is invalid: {0}")]
    Segment(#[source] EntryNameError),
}

/// Why an entry name is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum EntryNameError {
    /// Nothing remains after trimming whitespace.
    #[error("entry name cannot be empty")]
    Empty,
    /// The normalized UTF-8 representation exceeds 255 bytes.
    #[error("entry name is {actual_bytes} bytes; maximum is {maximum_bytes}")]
    TooLong {
        /// Actual normalized UTF-8 byte length.
        actual_bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
    /// NUL cannot appear in an entry name.
    #[error("entry name cannot contain NUL")]
    ContainsNul,
    /// Slash cannot appear because names are not storage paths.
    #[error("entry name cannot contain '/'")]
    ContainsSlash,
    /// Dot and dot-dot are reserved traversal segments.
    #[error("entry name cannot be '.' or '..'")]
    Reserved,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{
        EntryName, EntryNameError, EntryPath, EntryPathError, MAX_ENTRY_NAME_BYTES,
        MAX_ENTRY_PATH_BYTES,
    };

    #[test]
    fn names_are_trimmed_without_unicode_normalization() {
        let result = EntryName::new("  Quarterly report.md\n");
        assert_eq!(
            result.map(super::EntryName::into_inner),
            Ok("Quarterly report.md".to_owned())
        );
    }

    #[test]
    fn reserved_and_path_like_names_are_rejected() {
        assert_eq!(EntryName::new("."), Err(EntryNameError::Reserved));
        assert_eq!(EntryName::new(".."), Err(EntryNameError::Reserved));
        assert_eq!(EntryName::new("a/b"), Err(EntryNameError::ContainsSlash));
        assert_eq!(EntryName::new("a\0b"), Err(EntryNameError::ContainsNul));
    }

    #[test]
    fn byte_limit_applies_to_utf8_not_character_count() {
        let name = "é".repeat(128);
        assert_eq!(
            EntryName::new(name),
            Err(EntryNameError::TooLong {
                actual_bytes: 256,
                maximum_bytes: MAX_ENTRY_NAME_BYTES,
            })
        );
    }

    #[test]
    fn paths_normalize_the_contracted_url_shape() -> Result<(), EntryPathError> {
        let path = EntryPath::new("/private/cos:tos/top_secret/this_secret.md/")?;
        assert_eq!(path.as_str(), "private/cos:tos/top_secret/this_secret.md");
        assert_eq!(path.root_segment(), "private");
        assert_eq!(path.name(), "this_secret.md");
        assert_eq!(path.depth(), 4);
        assert!(!path.is_root());
        assert_eq!(
            path.parent().map(EntryPath::into_inner),
            Some("private/cos:tos/top_secret".to_owned())
        );
        Ok(())
    }

    #[test]
    fn root_paths_have_no_parent() -> Result<(), EntryPathError> {
        let path = EntryPath::new("public")?;
        assert!(path.is_root());
        assert_eq!(path.parent(), None);
        assert_eq!(path.root_segment(), "public");
        Ok(())
    }

    #[test]
    fn traversal_and_empty_segments_are_rejected() {
        assert_eq!(EntryPath::new("   "), Err(EntryPathError::Empty));
        assert_eq!(EntryPath::new("///"), Err(EntryPathError::Empty));
        assert!(matches!(
            EntryPath::new("public/../private"),
            Err(EntryPathError::Segment(EntryNameError::Reserved))
        ));
        assert!(matches!(
            EntryPath::new("public//child"),
            Err(EntryPathError::Segment(EntryNameError::Empty))
        ));
    }

    #[test]
    fn joining_respects_the_persisted_path_bound() -> Result<(), EntryPathError> {
        let name =
            EntryName::new("a".repeat(MAX_ENTRY_NAME_BYTES)).map_err(EntryPathError::Segment)?;
        let mut path = EntryPath::new("public")?;
        while let Ok(longer) = path.join(&name) {
            path = longer;
        }
        assert!(path.as_str().len() <= MAX_ENTRY_PATH_BYTES);
        assert!(matches!(
            path.join(&name),
            Err(EntryPathError::TooLong { .. })
        ));
        Ok(())
    }

    proptest! {
        #[test]
        fn accepted_names_always_satisfy_the_persisted_invariants(raw in ".{0,300}") {
            if let Ok(name) = EntryName::new(&raw) {
                prop_assert!(!name.as_str().is_empty());
                prop_assert!(name.as_str().len() <= MAX_ENTRY_NAME_BYTES);
                prop_assert_eq!(name.as_str(), name.as_str().trim());
                prop_assert!(!name.as_str().contains('/'));
                prop_assert!(!name.as_str().contains('\0'));
                prop_assert!(!matches!(name.as_str(), "." | ".."));
            }
        }
    }
}
