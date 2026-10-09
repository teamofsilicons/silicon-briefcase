//! Silicon Accounts identities: the permanent account uuid, its kind and its
//! changeable public id, plus the custodian rule that replaces "the same org".
//!
//! Briefcase keys everything on the account `uuid` (the access token's `sub`).
//! The public id (`c:ada`, `si:scout`) is for display and for callers to name
//! accounts; it can change, so it is never a key.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::actor::{ActorKind, is_canonical_iam_organization_id};

/// Shortest account uuid Silicon Accounts issues.
pub const MIN_ACCOUNT_UUID_BYTES: usize = 3;
/// Longest account uuid Briefcase accepts (Accounts issues at most 12 today).
pub const MAX_ACCOUNT_UUID_BYTES: usize = 32;
/// Longest public id handle (`c:` / `si:` excluded) Briefcase accepts.
pub const MAX_HANDLE_BYTES: usize = 64;

/// Why a value is not an account uuid or public id.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AccountIdentifierError {
    /// The uuid is not 3 to 32 ASCII letters and digits.
    #[error(
        "`{0}` is not a Silicon Accounts uuid: a uuid is 3 to 32 ASCII letters and digits, such as `zQo`"
    )]
    InvalidUuid(String),
    /// The id does not start with `c:` or `si:` followed by a handle.
    #[error(
        "`{0}` is not a Carbon or Silicon id: use `c:<handle>` for a Carbon or `si:<handle>` for a Silicon, such as `c:ada` or `si:scout`"
    )]
    InvalidPublicId(String),
}

/// The permanent, case-sensitive Silicon Accounts identifier (`sub`).
///
/// It is compared exactly: never lowercased, never parsed as an RFC 4122 UUID.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AccountUuid(String);

impl AccountUuid {
    /// Validates an account uuid.
    ///
    /// # Errors
    ///
    /// Returns [`AccountIdentifierError::InvalidUuid`] unless the value is 3 to
    /// 32 ASCII letters and digits.
    pub fn new(value: impl Into<String>) -> Result<Self, AccountIdentifierError> {
        let value = value.into();
        if is_account_uuid(&value) {
            Ok(Self(value))
        } else {
            Err(AccountIdentifierError::InvalidUuid(printable(&value)))
        }
    }

    /// Returns the uuid text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the uuid and returns its text.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for AccountUuid {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for AccountUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AccountUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Returns whether `value` has the shape of a Silicon Accounts uuid.
#[must_use]
pub fn is_account_uuid(value: &str) -> bool {
    (MIN_ACCOUNT_UUID_BYTES..=MAX_ACCOUNT_UUID_BYTES).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Returns whether `value` can name a storage tenant.
///
/// During the move to personal drives a tenant key is either an account uuid
/// (an account's own drive) or a legacy organization handle that still holds
/// pre-migration rows. Anything else is never a tenant.
#[must_use]
pub fn is_tenant_key(value: &str) -> bool {
    is_account_uuid(value) || is_canonical_iam_organization_id(value)
}

/// A Carbon or Silicon public id (`c:ada`, `si:scout`), normalized to lowercase.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PublicId(String);

impl PublicId {
    /// Parses a caller-supplied id. Surrounding whitespace is removed and the
    /// id is lowercased, because Silicon Accounts ids are case-insensitive.
    ///
    /// # Errors
    ///
    /// Returns [`AccountIdentifierError::InvalidPublicId`] unless the value is
    /// `c:` or `si:` followed by 1 to 64 of `a-z`, `0-9`, `_` and `-`.
    pub fn parse(value: &str) -> Result<Self, AccountIdentifierError> {
        let normalized = value.trim().to_ascii_lowercase();
        let handle = normalized
            .strip_prefix("c:")
            .or_else(|| normalized.strip_prefix("si:"));
        let valid = handle.is_some_and(|handle| {
            (1..=MAX_HANDLE_BYTES).contains(&handle.len())
                && handle.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        });
        if valid {
            Ok(Self(normalized))
        } else {
            Err(AccountIdentifierError::InvalidPublicId(printable(value)))
        }
    }

    /// Returns the kind the prefix names.
    #[must_use]
    pub fn kind(&self) -> ActorKind {
        if self.0.starts_with("si:") {
            ActorKind::Silicon
        } else {
            ActorKind::Carbon
        }
    }

    /// Returns the id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PublicId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How the caller's credential reached Briefcase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Credential {
    /// A Silicon Accounts access token issued to Briefcase (`Authorization: Bearer`).
    AccessToken {
        /// Token family (`fid`): one sign-in, across its refreshes.
        token_family: Option<String>,
        /// Membership id (`mid`, `briefcase:{uuid}`).
        membership_id: Option<String>,
        /// Space-separated scopes the account granted Briefcase.
        scope: Option<String>,
        /// Issued-at time, unix seconds (`iat`).
        issued_at: Option<i64>,
    },
    /// A User verification proof another app sent (`Authorization: Proof`).
    Proof {
        /// The app acting for the account.
        issuing_app: String,
        /// Proof family id, for logs and audit.
        proof_id: String,
        /// The one scope this route required and the proof carried.
        scope: String,
    },
}

/// The verified account behind one request.
///
/// `uuid` is the key; `id` is the public id the credential carried and is only
/// ever displayed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Actor {
    /// Permanent account uuid (`sub`, or the proof's `user.uuid`).
    pub uuid: AccountUuid,
    /// Carbon or Silicon.
    pub kind: ActorKind,
    /// Public id when the credential was issued (it may have changed since).
    pub id: Option<String>,
    /// How the credential reached Briefcase.
    pub credential: Credential,
}

impl Actor {
    /// Space-separated scopes of an access token; `None` for a proof.
    #[must_use]
    pub fn scope(&self) -> Option<&str> {
        match &self.credential {
            Credential::AccessToken { scope, .. } => scope.as_deref(),
            Credential::Proof { .. } => None,
        }
    }

    /// The sign-in's token family; `None` for a proof.
    #[must_use]
    pub fn token_family(&self) -> Option<&str> {
        match &self.credential {
            Credential::AccessToken { token_family, .. } => token_family.as_deref(),
            Credential::Proof { .. } => None,
        }
    }

    /// The app acting for the account, when the credential is a proof.
    #[must_use]
    pub fn issuing_app(&self) -> Option<&str> {
        match &self.credential {
            Credential::AccessToken { .. } => None,
            Credential::Proof { issuing_app, .. } => Some(issuing_app),
        }
    }
}

/// What Briefcase knows about one account's custodian relationship.
///
/// A Silicon always has exactly one custodian (a Carbon); a Carbon has none.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustodyFacts {
    /// The account.
    pub uuid: AccountUuid,
    /// Carbon or Silicon.
    pub kind: ActorKind,
    /// A Silicon's custodian, when known.
    pub custodian: Option<AccountUuid>,
}

impl CustodyFacts {
    /// The Carbon at the centre of this account's circle: a Carbon itself, or a
    /// Silicon's custodian. `None` for a Silicon whose custodian is unknown.
    #[must_use]
    pub fn anchor(&self) -> Option<&AccountUuid> {
        match self.kind {
            ActorKind::Carbon => Some(&self.uuid),
            ActorKind::Silicon => self.custodian.as_ref(),
        }
    }
}

/// Returns whether two accounts share a custodian circle.
///
/// For a Silicon S the circle is S, its custodian, and every other Silicon with
/// the same custodian. For a Carbon C it is C and every Silicon C is custodian
/// of. Two different Carbons are never in one circle. A Silicon whose
/// custodian is unknown is only in its own circle (fail closed).
#[must_use]
pub fn same_circle(left: &CustodyFacts, right: &CustodyFacts) -> bool {
    if left.uuid == right.uuid {
        return true;
    }
    match (left.anchor(), right.anchor()) {
        (Some(left_anchor), Some(right_anchor)) => left_anchor == right_anchor,
        _ => false,
    }
}

/// Returns whether `custodian` is the custodian of the Silicon `silicon`.
///
/// This is the custodian rule: a Carbon who looks after a Silicon may see and
/// manage that Silicon's things, but never acts as the Silicon.
#[must_use]
pub fn is_custodian_of(custodian: &AccountUuid, silicon: &CustodyFacts) -> bool {
    silicon.kind == ActorKind::Silicon && silicon.custodian.as_ref() == Some(custodian)
}

/// Derives a stable UUID (version 8) naming `value` within `namespace`.
///
/// Storage that keys rows on UUIDs (such as delegated-upload identity
/// columns) uses this to bind a row to an account uuid or tenant key without
/// ever parsing those case-sensitive strings as UUIDs themselves.
#[must_use]
pub fn storage_key(namespace: &str, value: &str) -> uuid::Uuid {
    use sha2::{Digest as _, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"silicon-briefcase/storage-key/v1\0");
    digest.update(namespace.as_bytes());
    digest.update(b"\0");
    digest.update(value.as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn printable(value: &str) -> String {
    value
        .chars()
        .take(80)
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AccountUuid, ActorKind, CustodyFacts, PublicId, is_custodian_of, is_tenant_key,
        same_circle, storage_key,
    };

    fn uuid(value: &str) -> AccountUuid {
        AccountUuid::new(value).unwrap_or_else(|error| panic!("test uuid {value}: {error}"))
    }

    fn carbon(id: &str) -> CustodyFacts {
        CustodyFacts {
            uuid: uuid(id),
            kind: ActorKind::Carbon,
            custodian: None,
        }
    }

    fn silicon(id: &str, custodian: Option<&str>) -> CustodyFacts {
        CustodyFacts {
            uuid: uuid(id),
            kind: ActorKind::Silicon,
            custodian: custodian.map(uuid),
        }
    }

    #[test]
    fn uuids_are_case_sensitive_base62_and_never_normalized() {
        assert_eq!(uuid("zQo").as_str(), "zQo");
        assert_ne!(uuid("zQo"), uuid("zqo"));
        assert!(AccountUuid::new("ab").is_err());
        assert!(AccountUuid::new("c:ada").is_err());
        assert!(AccountUuid::new("zQo ").is_err());
        assert!(AccountUuid::new("0192a3b4-0000-7000-8000-000000000000").is_err());
        let error = AccountUuid::new("bad uuid")
            .err()
            .map(|error| error.to_string());
        assert_eq!(
            error.as_deref(),
            Some(
                "`bad uuid` is not a Silicon Accounts uuid: a uuid is 3 to 32 ASCII letters and digits, such as `zQo`"
            )
        );
    }

    #[test]
    fn public_ids_are_trimmed_lowercased_and_typed_by_prefix() -> anyhow::Result<()> {
        let carbon = PublicId::parse(" C:Ada ")?;
        assert_eq!(carbon.as_str(), "c:ada");
        assert_eq!(carbon.kind(), ActorKind::Carbon);
        assert_eq!(PublicId::parse("si:scout_2")?.kind(), ActorKind::Silicon);
        for invalid in [
            "ada",
            "c:",
            "si:",
            "x:ada",
            "c:ada king",
            "c:ada/x",
            "tag:research",
        ] {
            assert!(PublicId::parse(invalid).is_err(), "{invalid}");
        }
        Ok(())
    }

    #[test]
    fn tenant_keys_are_account_uuids_or_legacy_org_handles() {
        assert!(is_tenant_key("zQo"));
        assert!(is_tenant_key("tos"));
        assert!(is_tenant_key("team-of-silicons"));
        assert!(!is_tenant_key("c:ada"));
        assert!(!is_tenant_key("a b"));
        assert!(!is_tenant_key(""));
    }

    #[test]
    fn storage_keys_are_stable_namespaced_and_case_sensitive() {
        let key = storage_key("account", "zQo");
        assert_eq!(key, storage_key("account", "zQo"));
        assert_ne!(key, storage_key("account", "zqo"));
        assert_ne!(key, storage_key("tenant", "zQo"));
        assert_eq!(key.get_version_num(), 8);
    }

    #[test]
    fn the_circle_is_a_carbon_and_the_silicons_it_looks_after() {
        let ada = carbon("Ada");
        let bob = carbon("Bob");
        let scout = silicon("Sc1", Some("Ada"));
        let pilot = silicon("Pl1", Some("Ada"));
        let rover = silicon("Rv1", Some("Bob"));
        let orphan = silicon("Or1", None);

        assert!(same_circle(&ada, &ada));
        assert!(same_circle(&ada, &scout));
        assert!(same_circle(&scout, &ada));
        assert!(same_circle(&scout, &pilot));
        assert!(!same_circle(&ada, &bob));
        assert!(!same_circle(&ada, &rover));
        assert!(!same_circle(&scout, &rover));
        assert!(!same_circle(&orphan, &ada));
        assert!(same_circle(&orphan, &orphan));
    }

    #[test]
    fn only_the_recorded_custodian_looks_after_a_silicon() {
        let scout = silicon("Sc1", Some("Ada"));
        assert!(is_custodian_of(&uuid("Ada"), &scout));
        assert!(!is_custodian_of(&uuid("Bob"), &scout));
        assert!(!is_custodian_of(&uuid("Ada"), &carbon("Ada")));
        assert!(!is_custodian_of(&uuid("Ada"), &silicon("Or1", None)));
    }
}
