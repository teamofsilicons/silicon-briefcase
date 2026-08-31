//! Authenticated actors and organization request context.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Maximum UTF-8 byte length of any identifier supplied by IAM.
pub const MAX_EXTERNAL_IDENTIFIER_BYTES: usize = 255;

/// An invalid identifier received from an external authority.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ExternalIdentifierError {
    /// Identifiers must contain at least one non-whitespace character.
    #[error("identifier cannot be empty")]
    Empty,
    /// Leading or trailing whitespace is rejected instead of normalized.
    #[error("identifier cannot have leading or trailing whitespace")]
    SurroundingWhitespace,
    /// Identifiers must fit request headers and the bounded persistence column.
    #[error("identifier is {actual_bytes} bytes; maximum is {maximum_bytes}")]
    TooLong {
        /// Actual UTF-8 byte length.
        actual_bytes: usize,
        /// Maximum accepted UTF-8 byte length.
        maximum_bytes: usize,
    },
    /// Control characters can corrupt request metadata, logs, and storage.
    #[error("identifier cannot contain control characters")]
    ContainsControlCharacter,
}

fn validate_external_identifier(value: String) -> Result<String, ExternalIdentifierError> {
    if value.trim().is_empty() {
        return Err(ExternalIdentifierError::Empty);
    }
    if value.trim() != value {
        return Err(ExternalIdentifierError::SurroundingWhitespace);
    }
    if value.len() > MAX_EXTERNAL_IDENTIFIER_BYTES {
        return Err(ExternalIdentifierError::TooLong {
            actual_bytes: value.len(),
            maximum_bytes: MAX_EXTERNAL_IDENTIFIER_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(ExternalIdentifierError::ContainsControlCharacter);
    }
    Ok(value)
}

macro_rules! external_identifier {
    ($(#[$metadata:meta])* $name:ident) => {
        $(#[$metadata])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validates and constructs the identifier.
            ///
            /// # Errors
            ///
            /// Returns [`ExternalIdentifierError`] when the value is empty,
            /// exceeds 255 UTF-8 bytes, contains a control character, or has
            /// leading or trailing whitespace.
            pub fn new(value: impl Into<String>) -> Result<Self, ExternalIdentifierError> {
                validate_external_identifier(value.into()).map(Self)
            }

            /// Returns the identifier as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns its text.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl TryFrom<String> for $name {
            type Error = ExternalIdentifierError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

external_identifier!(
    /// An IAM actor identifier.
    ActorId
);
external_identifier!(
    /// An immutable IAM organization identifier.
    OrganizationId
);
external_identifier!(
    /// An IAM application identifier.
    ApplicationId
);
external_identifier!(
    /// The exact IAM tag name used to derive tag-boundary access.
    TagName
);

/// A represented account type accepted by Briefcase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A human account.
    Carbon,
    /// An AI-agent account.
    Silicon,
}

/// A stable reference to the represented Carbon or Silicon.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ActorRef {
    kind: ActorKind,
    id: ActorId,
}

impl ActorRef {
    /// Constructs an actor reference from trusted IAM identity data.
    #[must_use]
    pub const fn new(kind: ActorKind, id: ActorId) -> Self {
        Self { kind, id }
    }

    /// Returns the account type.
    #[must_use]
    pub const fn kind(&self) -> ActorKind {
        self.kind
    }

    /// Returns the IAM actor identifier.
    #[must_use]
    pub const fn id(&self) -> &ActorId {
        &self.id
    }
}

/// The represented actor's current IAM organization role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    /// A regular organization member.
    Member,
    /// An organization administrator.
    Admin,
    /// The organization owner.
    Owner,
}

impl OrganizationRole {
    /// Returns whether the role receives Briefcase administrative access.
    #[must_use]
    pub const fn has_administrative_access(self) -> bool {
        matches!(self, Self::Admin | Self::Owner)
    }
}

/// How IAM authenticated the represented actor for this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum AuthenticationMode {
    /// The actor called Briefcase with an IAM bearer token.
    Bearer,
    /// An IAM-verified application is acting on behalf of the actor.
    OnBehalfOf {
        /// The verified originating IAM application.
        application_id: ApplicationId,
    },
}

impl AuthenticationMode {
    /// Returns the verified originating application for an OBO request.
    #[must_use]
    pub const fn originating_application(&self) -> Option<&ApplicationId> {
        match self {
            Self::Bearer => None,
            Self::OnBehalfOf { application_id } => Some(application_id),
        }
    }
}

/// Trusted authorization facts established through online IAM verification.
///
/// Constructing this value does not perform IAM verification. Only an IAM
/// adapter that has already validated active membership, organization, role,
/// tags, and any OBO proof should construct it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAuthContext {
    organization_id: OrganizationId,
    actor: ActorRef,
    role: OrganizationRole,
    tags: BTreeSet<TagName>,
    authentication: AuthenticationMode,
}

impl RequestAuthContext {
    /// Constructs a request context from IAM-verified facts.
    #[must_use]
    pub fn new(
        organization_id: OrganizationId,
        actor: ActorRef,
        role: OrganizationRole,
        tags: impl IntoIterator<Item = TagName>,
        authentication: AuthenticationMode,
    ) -> Self {
        Self {
            organization_id,
            actor,
            role,
            tags: tags.into_iter().collect(),
            authentication,
        }
    }

    /// Returns the verified organization.
    #[must_use]
    pub const fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    /// Returns the represented actor.
    #[must_use]
    pub const fn actor(&self) -> &ActorRef {
        &self.actor
    }

    /// Returns the actor's current organization role.
    #[must_use]
    pub const fn role(&self) -> OrganizationRole {
        self.role
    }

    /// Returns the actor's current IAM tags.
    #[must_use]
    pub const fn tags(&self) -> &BTreeSet<TagName> {
        &self.tags
    }

    /// Returns whether IAM currently assigns the exact tag to the actor.
    #[must_use]
    pub fn has_tag(&self, tag: &TagName) -> bool {
        self.tags.contains(tag)
    }

    /// Returns the verified authentication mode.
    #[must_use]
    pub const fn authentication(&self) -> &AuthenticationMode {
        &self.authentication
    }

    /// Returns the verified originating application for an OBO request.
    #[must_use]
    pub const fn originating_application(&self) -> Option<&ApplicationId> {
        self.authentication.originating_application()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActorId, ApplicationId, ExternalIdentifierError, MAX_EXTERNAL_IDENTIFIER_BYTES,
        OrganizationId, OrganizationRole, TagName,
    };

    #[test]
    fn external_identifiers_reject_ambiguous_whitespace() {
        assert_eq!(ActorId::new(""), Err(ExternalIdentifierError::Empty));
        assert_eq!(
            ActorId::new(" carbon-a"),
            Err(ExternalIdentifierError::SurroundingWhitespace)
        );
    }

    #[test]
    fn every_external_identifier_enforces_the_persistence_byte_limit() {
        let maximum = "a".repeat(MAX_EXTERNAL_IDENTIFIER_BYTES);
        assert!(ActorId::new(maximum.clone()).is_ok());
        assert!(OrganizationId::new(maximum.clone()).is_ok());
        assert!(ApplicationId::new(maximum.clone()).is_ok());
        assert!(TagName::new(maximum).is_ok());

        let too_long = "é".repeat(128);
        let expected = ExternalIdentifierError::TooLong {
            actual_bytes: 256,
            maximum_bytes: MAX_EXTERNAL_IDENTIFIER_BYTES,
        };
        assert_eq!(ActorId::new(too_long.clone()), Err(expected.clone()));
        assert_eq!(OrganizationId::new(too_long.clone()), Err(expected.clone()));
        assert_eq!(ApplicationId::new(too_long.clone()), Err(expected.clone()));
        assert_eq!(TagName::new(too_long), Err(expected));
    }

    #[test]
    fn every_external_identifier_rejects_control_characters() {
        assert_eq!(
            ActorId::new("actor\nforged"),
            Err(ExternalIdentifierError::ContainsControlCharacter)
        );
        assert_eq!(
            OrganizationId::new("org\u{7f}hidden"),
            Err(ExternalIdentifierError::ContainsControlCharacter)
        );
        assert_eq!(
            ApplicationId::new("app\u{0085}hidden"),
            Err(ExternalIdentifierError::ContainsControlCharacter)
        );
        assert_eq!(
            TagName::new("tag\0hidden"),
            Err(ExternalIdentifierError::ContainsControlCharacter)
        );
    }

    #[test]
    fn only_administrative_roles_are_elevated() {
        assert!(!OrganizationRole::Member.has_administrative_access());
        assert!(OrganizationRole::Admin.has_administrative_access());
        assert!(OrganizationRole::Owner.has_administrative_access());
    }
}
