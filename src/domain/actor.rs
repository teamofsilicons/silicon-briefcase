//! Authenticated actors and the tenant request context.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use super::account::Actor;

/// Maximum UTF-8 byte length of any identifier supplied by an external authority.
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
    /// An actor identifier (an account uuid, or a legacy public id in old rows).
    ActorId
);
external_identifier!(
    /// A tenant key (an account uuid, or a legacy organization handle).
    OrganizationId
);
external_identifier!(
    /// A Silicon Apps application identifier.
    ApplicationId
);
external_identifier!(
    /// A legacy tag name kept only to read pre-migration rows.
    TagName
);

/// Returns whether a string is a canonical bare application ID, such as `briefcase`.
#[must_use]
pub fn is_canonical_iam_application_id(value: &str) -> bool {
    (1..=80).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

/// Returns whether a string is a legacy organization handle (lowercase, 3 to 50 characters).
#[must_use]
pub fn is_canonical_iam_organization_id(value: &str) -> bool {
    (3..=50).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

/// A represented account type accepted by Briefcase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A Carbon's account (`c:` ids).
    Carbon,
    /// A Silicon's account (`si:` ids); it always has one custodian Carbon.
    Silicon,
}

/// A stable reference to the represented Carbon or Silicon.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ActorRef {
    kind: ActorKind,
    id: ActorId,
}

impl ActorRef {
    /// Constructs an actor reference from verified identity data.
    #[must_use]
    pub const fn new(kind: ActorKind, id: ActorId) -> Self {
        Self { kind, id }
    }

    /// Returns the account type.
    #[must_use]
    pub const fn kind(&self) -> ActorKind {
        self.kind
    }

    /// Returns the actor identifier.
    #[must_use]
    pub const fn id(&self) -> &ActorId {
        &self.id
    }
}

/// How the caller relates to the drive a request acts in.
///
/// Every account owns one drive. Its owner, and the custodian of a Silicon
/// that owns it, hold the owner's rights over everything in it; anyone else
/// reaches only what was shared with them and what they (or the Silicons they
/// look after) created there.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveAccess {
    /// The drive is the caller's own.
    Owner,
    /// The caller is the custodian of the Silicon that owns the drive: it sees
    /// and manages the Silicon's files, but never acts as the Silicon.
    Custodian,
    /// Anyone else.
    Visitor,
}

impl DriveAccess {
    /// Returns whether the caller holds the drive owner's rights over the
    /// drive's content.
    #[must_use]
    pub const fn has_owner_rights(self) -> bool {
        matches!(self, Self::Owner | Self::Custodian)
    }
}

/// How the represented actor was authenticated for this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum AuthenticationMode {
    /// The actor called Briefcase with its own access token.
    Bearer,
    /// Another app is acting for the actor with a verified proof. It reaches
    /// only its own folder, `apps/{app}/`, in the actor's own drive.
    OnBehalfOf {
        /// The verified issuing application.
        application_id: ApplicationId,
    },
    /// A first-party app the operator lists is acting for the actor with a
    /// verified proof. It reaches everything the actor can (its own drive,
    /// what was shared with it, the drives of the Silicons it looks after),
    /// still only through the delegated routes its proof's scopes name.
    FirstPartyOnBehalfOf {
        /// The verified issuing application.
        application_id: ApplicationId,
    },
}

impl AuthenticationMode {
    /// Returns the verified issuing application for a delegated request.
    #[must_use]
    pub const fn originating_application(&self) -> Option<&ApplicationId> {
        match self {
            Self::Bearer => None,
            Self::OnBehalfOf { application_id } | Self::FirstPartyOnBehalfOf { application_id } => {
                Some(application_id)
            }
        }
    }

    /// Returns the issuing application when it is confined to its own
    /// `apps/{app}/` folder.
    #[must_use]
    pub const fn confined_application(&self) -> Option<&ApplicationId> {
        match self {
            Self::OnBehalfOf { application_id } => Some(application_id),
            Self::Bearer | Self::FirstPartyOnBehalfOf { .. } => None,
        }
    }
}

/// Trusted authorization facts for one request: who is calling, the drive the
/// request acts in, and the caller's rights there.
///
/// Constructing this value does not verify anything. The authentication
/// boundary builds it with [`RequestAuthContext::for_account`] after it
/// verified a Silicon Accounts access token or proof (the caller's own drive),
/// and the drive resolution moves it into another drive with
/// [`RequestAuthContext::in_drive`] once it established the caller's relation
/// to that drive's owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestAuthContext {
    account: Option<Actor>,
    organization_id: OrganizationId,
    drive_owner: ActorRef,
    actor: ActorRef,
    access: DriveAccess,
    creators: BTreeSet<ActorId>,
    authentication: AuthenticationMode,
}

impl RequestAuthContext {
    /// Constructs the context of `actor` acting in the drive of
    /// `drive_owner`, whose account uuid is the drive key.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalIdentifierError`] when the owner's id cannot be a
    /// drive key.
    pub fn for_drive(
        drive_owner: ActorRef,
        actor: ActorRef,
        access: DriveAccess,
        authentication: AuthenticationMode,
    ) -> Result<Self, ExternalIdentifierError> {
        let organization_id = OrganizationId::new(drive_owner.id().as_str())?;
        let creators = BTreeSet::from([actor.id().clone()]);
        Ok(Self {
            account: None,
            organization_id,
            drive_owner,
            actor,
            access,
            creators,
            authentication,
        })
    }

    /// Constructs the context of a verified Silicon Accounts actor acting in
    /// its own drive.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalIdentifierError`] when the uuid or the issuing app
    /// cannot be represented as identifiers (never for values that passed
    /// Silicon Accounts verification).
    pub fn for_account(account: Actor) -> Result<Self, ExternalIdentifierError> {
        let actor = ActorRef::new(account.kind, ActorId::new(account.uuid.as_str())?);
        let authentication = match (account.issuing_app(), account.confined_app()) {
            (Some(app), Some(_)) => AuthenticationMode::OnBehalfOf {
                application_id: ApplicationId::new(app)?,
            },
            (Some(app), None) => AuthenticationMode::FirstPartyOnBehalfOf {
                application_id: ApplicationId::new(app)?,
            },
            (None, _) => AuthenticationMode::Bearer,
        };
        let mut context =
            Self::for_drive(actor.clone(), actor, DriveAccess::Owner, authentication)?;
        context.account = Some(account);
        Ok(context)
    }

    /// Moves the same caller into the drive of `drive_owner`.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalIdentifierError`] when the owner's id cannot be a
    /// drive key.
    pub fn in_drive(
        &self,
        drive_owner: ActorRef,
        access: DriveAccess,
    ) -> Result<Self, ExternalIdentifierError> {
        let organization_id = OrganizationId::new(drive_owner.id().as_str())?;
        Ok(Self {
            organization_id,
            drive_owner,
            access,
            ..self.clone()
        })
    }

    /// Adds the Silicons the caller looks after. Whatever they created, in
    /// any drive, carries creator rights for the caller too: the custodian
    /// rule.
    #[must_use]
    pub fn with_custodial_silicons(mut self, silicons: impl IntoIterator<Item = ActorId>) -> Self {
        self.creators.extend(silicons);
        self
    }

    /// Returns the verified Silicon Accounts actor, when the request carried
    /// a Silicon Accounts credential.
    #[must_use]
    pub const fn account(&self) -> Option<&Actor> {
        self.account.as_ref()
    }

    /// Returns the drive the request acts in (its owner's uuid).
    #[must_use]
    pub const fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    /// Returns the account that owns the drive the request acts in.
    #[must_use]
    pub const fn drive_owner(&self) -> &ActorRef {
        &self.drive_owner
    }

    /// Returns the represented actor.
    #[must_use]
    pub const fn actor(&self) -> &ActorRef {
        &self.actor
    }

    /// Returns how the caller relates to the drive.
    #[must_use]
    pub const fn access(&self) -> DriveAccess {
        self.access
    }

    /// Returns whether the caller holds the drive owner's rights here.
    #[must_use]
    pub const fn has_owner_rights(&self) -> bool {
        self.access.has_owner_rights()
    }

    /// Returns whether the caller holds creator rights over what `owner`
    /// created: its own entries and those of the Silicons it looks after.
    #[must_use]
    pub fn is_creator(&self, owner: &ActorRef) -> bool {
        self.creators.contains(owner.id())
    }

    /// Returns the accounts whose entries carry creator rights for the caller,
    /// the caller first.
    #[must_use]
    pub fn creator_ids(&self) -> Vec<String> {
        let mut ids = vec![self.actor.id().as_str().to_owned()];
        ids.extend(
            self.creators
                .iter()
                .filter(|id| *id != self.actor.id())
                .map(|id| id.as_str().to_owned()),
        );
        ids
    }

    /// Returns the verified authentication mode.
    #[must_use]
    pub const fn authentication(&self) -> &AuthenticationMode {
        &self.authentication
    }

    /// Returns the verified originating application for a delegated request.
    #[must_use]
    pub const fn originating_application(&self) -> Option<&ApplicationId> {
        self.authentication.originating_application()
    }

    /// Returns the originating application when it is confined to its own
    /// `apps/{app}/` folder (every issuer but the listed first-party ones).
    #[must_use]
    pub const fn confined_application(&self) -> Option<&ApplicationId> {
        self.authentication.confined_application()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActorId, ActorKind, ActorRef, ApplicationId, AuthenticationMode, DriveAccess,
        ExternalIdentifierError, MAX_EXTERNAL_IDENTIFIER_BYTES, OrganizationId, RequestAuthContext,
        TagName, is_canonical_iam_application_id, is_canonical_iam_organization_id,
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
    fn iam_application_and_organization_ids_are_canonical() {
        assert!(is_canonical_iam_application_id("briefcase"));
        assert!(is_canonical_iam_application_id("briefcase_api-1"));
        assert!(!is_canonical_iam_application_id("2briefcase"));
        assert!(!is_canonical_iam_application_id("tos>2briefcase"));
        assert!(!is_canonical_iam_application_id("tos>Briefcase"));
        assert!(!is_canonical_iam_application_id("briefcase>other"));
        assert!(!is_canonical_iam_application_id("to>briefcase"));
        assert!(is_canonical_iam_application_id("ab"));
        assert!(is_canonical_iam_organization_id("team-of-silicons"));
        assert!(!is_canonical_iam_organization_id("Team"));
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
    fn only_owners_and_custodians_hold_owner_rights() {
        assert!(DriveAccess::Owner.has_owner_rights());
        assert!(DriveAccess::Custodian.has_owner_rights());
        assert!(!DriveAccess::Visitor.has_owner_rights());
    }

    #[test]
    fn a_caller_moves_between_drives_keeping_its_creator_rights() -> anyhow::Result<()> {
        let ada = ActorRef::new(ActorKind::Carbon, ActorId::new("Ada1")?);
        let scout = ActorRef::new(ActorKind::Silicon, ActorId::new("Sc1")?);
        let bob = ActorRef::new(ActorKind::Carbon, ActorId::new("Bob1")?);
        let own = RequestAuthContext::for_drive(
            ada.clone(),
            ada.clone(),
            DriveAccess::Owner,
            AuthenticationMode::Bearer,
        )?
        .with_custodial_silicons([scout.id().clone()]);
        assert_eq!(own.organization_id().as_str(), "Ada1");
        assert!(own.is_creator(&ada) && own.is_creator(&scout) && !own.is_creator(&bob));
        assert_eq!(own.creator_ids(), vec!["Ada1".to_owned(), "Sc1".to_owned()]);

        let visiting = own.in_drive(bob.clone(), DriveAccess::Visitor)?;
        assert_eq!(visiting.organization_id().as_str(), "Bob1");
        assert_eq!(visiting.drive_owner(), &bob);
        assert_eq!(visiting.actor(), &ada);
        assert!(!visiting.has_owner_rights());
        assert!(visiting.is_creator(&scout));
        Ok(())
    }
}
