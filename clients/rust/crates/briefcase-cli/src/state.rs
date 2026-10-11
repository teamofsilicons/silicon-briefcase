//! What the CLI remembers between runs.
//!
//! The package underneath is stateless; this is the part that is not. Profiles
//! and sign-ins live under `{home}/.briefcase/` (`$BRIEFCASE_HOME`, the
//! `briefcase config home` pointer, `$SILICON_HOME/.briefcase` or
//! `$HOME/.briefcase`). Sign-ins are in a file only the owner can read
//! (`credentials.json`, mode 0600), replaced atomically, and every
//! read-modify-write — above all a refresh, whose refresh token works once —
//! runs under an exclusive lock (`credentials.lock`).

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use briefcase_client::{AccountKind, AccountRef, IdempotencyKey};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Directory name under the home.
const STATE_DIRECTORY: &str = ".briefcase";
/// Pointer in the default home directory to a user-selected home directory.
const HOME_POINTER_FILE: &str = ".briefcase-home";
/// Profiles and their deployments.
const CONFIG_FILE: &str = "config.json";
/// Sign-ins, readable only by their owner.
const CREDENTIALS_FILE: &str = "credentials.json";
/// Cross-process lock protecting state read/modify/write transactions.
const CREDENTIALS_LOCK_FILE: &str = "credentials.lock";
/// The credentials file format written by this release.
const CREDENTIALS_FORMAT: u32 = 2;
/// Refresh when less than this is left of the access token.
pub const REFRESH_MARGIN: time::Duration = time::Duration::seconds(60);

/// Something the CLI could not read or write locally.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// The home directory could not be determined.
    #[error(
        "no home directory: set SILICON_HOME or HOME, or configure one with `briefcase config home <directory>`"
    )]
    NoHome,
    /// A configured home path exists but is not a directory.
    #[error("{path}: not a directory")]
    NotDirectory {
        /// The path supplied by the user or stored in the home pointer.
        path: String,
    },
    /// A state file could not be read or written.
    #[error("{path} could not be {action}: {source}")]
    File {
        /// The file involved.
        path: String,
        /// What was being attempted.
        action: &'static str,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
    /// A state file exists but does not parse.
    #[error("{path} is not valid Briefcase state: {reason}")]
    Malformed {
        /// The file involved.
        path: String,
        /// What is wrong with it.
        reason: String,
    },
}

/// One saved deployment.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Profile {
    /// Versioned API base URL.
    pub url: String,
    /// The Silicon Accounts URL this profile signs in at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accounts_url: Option<String>,
}

/// Every saved profile and which one is current.
///
/// Keys from older releases (`telemetry`, `auto_update`) are ignored when read
/// and dropped on the next write.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Configuration {
    /// Profile used when `--profile` is not given.
    #[serde(default = "default_profile_name")]
    pub current_profile: String,
    /// Saved profiles by name.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            current_profile: default_profile_name(),
            profiles: BTreeMap::new(),
        }
    }
}

fn default_profile_name() -> String {
    "default".to_owned()
}

/// How a sign-in was made.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInMethod {
    /// A Carbon approved a device code.
    Device,
    /// A short-lived token from `silicon-accounts login --app briefcase -q`.
    ShortLivedToken,
}

/// The account a sign-in is for.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredAccount {
    /// The permanent, case-sensitive account identifier.
    pub uuid: String,
    /// The public id at sign-in (`c:ada`); refreshed from Briefcase when it verifies.
    pub id: String,
    /// Carbon or Silicon.
    pub kind: AccountKind,
    /// The display name, when the account has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// A Silicon's custodian.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custodian: Option<AccountRef>,
}

impl StoredAccount {
    /// The public id, or the uuid while the account has none.
    #[must_use]
    pub fn display_id(&self) -> &str {
        if self.id.is_empty() {
            &self.uuid
        } else {
            &self.id
        }
    }
}

/// One Silicon Accounts sign-in to Briefcase.
#[derive(Clone, Deserialize, Serialize)]
pub struct StoredSession {
    /// The Briefcase API this sign-in is for; its tokens are never sent elsewhere.
    pub api_url: String,
    /// The Silicon Accounts it was made at; refresh and sign-out go there.
    pub accounts_url: String,
    /// The app the tokens are for (`briefcase`).
    pub app_id: String,
    /// Who signed in.
    pub account: StoredAccount,
    /// The 30-minute access token (`aud = briefcase`).
    pub access_token: String,
    /// The rotating refresh token (`sar_…`); a used one ends the whole sign-in.
    pub refresh_token: String,
    /// When the access token expires.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// When the refresh token family ends.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub refresh_expires_at: Option<OffsetDateTime>,
    /// The details the account shares with Briefcase.
    #[serde(default)]
    pub scope: Vec<String>,
    /// When the sign-in was made.
    #[serde(with = "time::serde::rfc3339")]
    pub signed_in_at: OffsetDateTime,
    /// Device flow or short-lived token.
    pub method: SignInMethod,
}

impl StoredSession {
    /// Whether the access token should be refreshed before the next request.
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= OffsetDateTime::now_utc() + REFRESH_MARGIN
    }

    /// Whether the access token still has more than `margin` to live.
    #[must_use]
    pub fn usable_for(&self, margin: time::Duration) -> bool {
        self.expires_at > OffsetDateTime::now_utc() + margin
    }
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredSession")
            .field("api_url", &self.api_url)
            .field("accounts_url", &self.accounts_url)
            .field("account", &self.account)
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

/// Why a profile is no longer signed in, kept so `login status` can say so.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EndedSession {
    /// Stable reason code, such as `sign_in_ended`.
    pub reason: String,
    /// Silicon Accounts' or Briefcase's own explanation.
    pub message: String,
    /// The account that was signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// When the CLI found out.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A profile signed in before Briefcase moved to Silicon Accounts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacySignIns {
    /// Profile name → the public id it was signed in as, when known.
    pub profiles: BTreeMap<String, Option<String>>,
}

/// Sign-ins and retry identities.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Credentials {
    /// File format; this release writes 2.
    #[serde(default)]
    pub format: u32,
    /// Sign-ins by profile.
    #[serde(default)]
    pub sessions: BTreeMap<String, StoredSession>,
    /// Why a profile's sign-in ended.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ended: BTreeMap<String, EndedSession>,
    /// Crash-recovery identity for path-addressed mutations.
    #[serde(default)]
    pub pending_mutations: BTreeMap<String, PendingMutation>,
    /// What an older file held, when it was written before Silicon Accounts.
    #[serde(skip)]
    pub legacy: Option<LegacySignIns>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("format", &self.format)
            .field("sessions", &self.sessions)
            .field("ended", &self.ended)
            .field("pending_mutation_scopes", &self.pending_mutations.keys())
            .field("legacy", &self.legacy)
            .finish()
    }
}

/// Durable retry identity for an in-flight CLI mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingMutation {
    /// Caller-owned key sent with every attempt of this exact operation.
    pub idempotency_key: String,
    /// SHA-256 of the complete user intent; raw secrets are never persisted.
    pub request_fingerprint: String,
    /// Stable entry resolved before a path-addressed mutation was attempted,
    /// so a retry finds it even after the first attempt moved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<Uuid>,
    /// Stable destination folder resolved before a path-addressed move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_id: Option<Uuid>,
}

impl Credentials {
    /// The sign-in saved for a profile.
    #[must_use]
    pub fn session(&self, profile: &str) -> Option<&StoredSession> {
        self.sessions.get(profile)
    }

    /// Saves a profile's sign-in, forgetting why an earlier one ended.
    pub fn set_session(&mut self, profile: &str, session: StoredSession) {
        self.ended.remove(profile);
        self.sessions.insert(profile.to_owned(), session);
    }

    /// Forgets a profile's sign-in, returning it.
    pub fn remove_session(&mut self, profile: &str) -> Option<StoredSession> {
        self.sessions.remove(profile)
    }

    /// Forgets a profile's sign-in because it ended, and remembers why.
    pub fn end_session(&mut self, profile: &str, reason: &str, message: &str) {
        let account_id = self
            .sessions
            .remove(profile)
            .map(|session| session.account.display_id().to_owned());
        self.ended.insert(
            profile.to_owned(),
            EndedSession {
                reason: reason.to_owned(),
                message: message.to_owned(),
                account_id,
                at: OffsetDateTime::now_utc(),
            },
        );
    }

    /// Returns a pending retry for one exact command scope.
    #[must_use]
    pub fn pending_mutation(&self, scope: &str) -> Option<&PendingMutation> {
        self.pending_mutations.get(scope)
    }

    /// Persists or replaces a pending retry before its network attempt.
    pub fn set_pending_mutation(&mut self, scope: String, pending: PendingMutation) {
        self.pending_mutations.insert(scope, pending);
    }

    /// Returns the durable retry identity for this intent, creating one when
    /// this is its first attempt or when the command's intent changed.
    ///
    /// The caller must save the credentials before making the network request.
    pub fn prepare_mutation(&mut self, scope: &str, request_fingerprint: &str) -> PendingMutation {
        if let Some(pending) = self.pending_mutation(scope)
            && pending.request_fingerprint == request_fingerprint
        {
            return pending.clone();
        }
        let pending = PendingMutation {
            idempotency_key: IdempotencyKey::random().as_str().to_owned(),
            request_fingerprint: request_fingerprint.to_owned(),
            resource_id: None,
            destination_id: None,
        };
        self.set_pending_mutation(scope.to_owned(), pending.clone());
        pending
    }

    /// Clears only the retry identity that produced a committed local result.
    pub fn clear_pending_mutation(&mut self, scope: &str, idempotency_key: &str) {
        if self
            .pending_mutations
            .get(scope)
            .is_some_and(|pending| pending.idempotency_key == idempotency_key)
        {
            self.pending_mutations.remove(scope);
        }
    }

    /// Reads a credentials file. A file written before Briefcase moved to
    /// Silicon Accounts (no `format`) holds nothing usable: its sign-ins are
    /// remembered only by profile and public id, so `login status` can say
    /// why the profile is signed out, and the next save drops them.
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        match value.get("format").and_then(serde_json::Value::as_u64) {
            Some(format) if format == u64::from(CREDENTIALS_FORMAT) => {
                serde_json::from_value(value).map_err(|error| error.to_string())
            }
            Some(format) => Err(format!(
                "format {format} was written by a newer briefcase; update briefcase (Silicon Apps keeps it current) or sign in again"
            )),
            None => {
                let mut legacy = LegacySignIns::default();
                if let Some(sessions) = value.get("sessions").and_then(serde_json::Value::as_object)
                {
                    for (profile, session) in sessions {
                        let id = session
                            .pointer("/actor/public_id")
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned);
                        legacy.profiles.insert(profile.clone(), id);
                    }
                }
                if let Some(tokens) = value.get("tokens").and_then(serde_json::Value::as_object) {
                    for profile in tokens.keys() {
                        legacy.profiles.entry(profile.clone()).or_insert(None);
                    }
                }
                Ok(Self {
                    format: CREDENTIALS_FORMAT,
                    legacy: Some(legacy),
                    ..Self::default()
                })
            }
        }
    }
}

/// The on-disk state directory.
#[derive(Clone, Debug)]
pub struct StateDirectory {
    root: PathBuf,
}

/// Exclusive cross-process ownership of mutable CLI state.
///
/// Closing the file releases the advisory lock, including during unwinding.
#[derive(Debug)]
pub struct CredentialsLock {
    file: File,
}

impl Drop for CredentialsLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

/// The shared Silicon home is the default parent for Briefcase state and its
/// configured-home pointer. The app-specific state override is resolved first.
fn default_home() -> Result<PathBuf, StateError> {
    let home = std::env::var_os("SILICON_HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|home| !home.is_empty()))
        .map(PathBuf::from)
        .ok_or(StateError::NoHome)?;
    if home.exists() && !home.is_dir() {
        return Err(StateError::NotDirectory {
            path: home.display().to_string(),
        });
    }
    Ok(home)
}

impl StateDirectory {
    /// Locates the state directory, honoring `BRIEFCASE_HOME` when set.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::NoHome`] when neither `BRIEFCASE_HOME`,
    /// `SILICON_HOME` nor `HOME` is available.
    pub fn locate() -> Result<Self, StateError> {
        if let Ok(explicit) = std::env::var("BRIEFCASE_HOME")
            && !explicit.trim().is_empty()
        {
            return Ok(Self {
                root: PathBuf::from(explicit),
            });
        }
        let home = default_home()?;
        let pointer = home.join(HOME_POINTER_FILE);
        if pointer.is_file() {
            let configured =
                std::fs::read_to_string(&pointer).map_err(|source| StateError::File {
                    path: pointer.display().to_string(),
                    action: "read",
                    source,
                })?;
            let configured = PathBuf::from(configured.trim());
            if !configured.is_dir() {
                return Err(StateError::NotDirectory {
                    path: configured.display().to_string(),
                });
            }
            return Ok(Self {
                root: configured.join(STATE_DIRECTORY),
            });
        }
        Ok(Self {
            root: home.join(STATE_DIRECTORY),
        })
    }

    /// Persists the user-selected parent directory for future invocations.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::NotDirectory`] when `directory` is not an existing
    /// directory, and a file error when the pointer cannot be written.
    pub fn configure_home(directory: impl AsRef<Path>) -> Result<(), StateError> {
        let directory = directory.as_ref();
        if !directory.is_dir() {
            return Err(StateError::NotDirectory {
                path: directory.display().to_string(),
            });
        }
        let home = default_home()?;
        std::fs::create_dir_all(&home).map_err(|source| StateError::File {
            path: home.display().to_string(),
            action: "created",
            source,
        })?;
        let pointer = home.join(HOME_POINTER_FILE);
        std::fs::write(
            &pointer,
            directory
                .canonicalize()
                .unwrap_or_else(|_| directory.to_owned())
                .display()
                .to_string(),
        )
        .map_err(|source| StateError::File {
            path: pointer.display().to_string(),
            action: "written",
            source,
        })?;
        restrict_to_owner(&pointer, 0o600)?;
        Ok(())
    }

    /// Uses an explicit state directory without changing process environment.
    #[cfg(test)]
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Reads the saved profiles, or defaults when nothing is saved yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn configuration(&self) -> Result<Configuration, StateError> {
        let path = self.root.join(CONFIG_FILE);
        match self.read(CONFIG_FILE)? {
            None => Ok(Configuration::default()),
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|error| StateError::Malformed {
                path: path.display().to_string(),
                reason: error.to_string(),
            }),
        }
    }

    /// Reads the saved sign-ins, or nothing when none is saved yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn credentials(&self) -> Result<Credentials, StateError> {
        let path = self.root.join(CREDENTIALS_FILE);
        match self.read(CREDENTIALS_FILE)? {
            None => Ok(Credentials {
                format: CREDENTIALS_FORMAT,
                ..Credentials::default()
            }),
            Some(bytes) => Credentials::parse(&bytes).map_err(|reason| StateError::Malformed {
                path: path.display().to_string(),
                reason,
            }),
        }
    }

    /// Serializes a state read/modify/write sequence across CLI processes.
    ///
    /// A refresh keeps the guard alive for its network request, so two CLI
    /// processes never present the same single-use refresh token.
    ///
    /// # Errors
    ///
    /// Returns an error when the private state directory or lock file cannot
    /// be created, secured, or locked.
    pub fn lock_credentials(&self) -> Result<CredentialsLock, StateError> {
        self.ensure_directory()?;
        let path = self.root.join(CREDENTIALS_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| StateError::File {
                path: path.display().to_string(),
                action: "opened",
                source,
            })?;
        restrict_to_owner(&path, 0o600)?;
        file.lock_exclusive().map_err(|source| StateError::File {
            path: path.display().to_string(),
            action: "locked",
            source,
        })?;
        Ok(CredentialsLock { file })
    }

    /// Writes the profiles.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save_configuration(&self, configuration: &Configuration) -> Result<(), StateError> {
        self.write_json(CONFIG_FILE, configuration)
    }

    /// Writes the sign-ins, readable only by their owner, in the current format.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save_credentials(&self, credentials: &Credentials) -> Result<(), StateError> {
        let mut current = credentials.clone();
        current.format = CREDENTIALS_FORMAT;
        self.write_json(CREDENTIALS_FILE, &current)
    }

    /// Moves an unreadable state file aside (`<name>.unreadable-<unix time>`)
    /// so the next write starts fresh; nothing is deleted. Returns where it went.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be renamed.
    pub fn set_aside_unreadable(&self, name: &str) -> Result<Option<PathBuf>, StateError> {
        let path = self.root.join(name);
        if !path.exists() {
            return Ok(None);
        }
        let aside = self.root.join(format!(
            "{name}.unreadable-{}",
            OffsetDateTime::now_utc().unix_timestamp()
        ));
        std::fs::rename(&path, &aside).map_err(|source| StateError::File {
            path: path.display().to_string(),
            action: "moved aside",
            source,
        })?;
        Ok(Some(aside))
    }

    /// Returns where the sign-ins live, so a message can name the file.
    #[must_use]
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join(CREDENTIALS_FILE)
    }

    /// The file names of the two state files, for [`Self::set_aside_unreadable`].
    pub const CONFIG: &'static str = CONFIG_FILE;
    /// See [`Self::CONFIG`].
    pub const CREDENTIALS: &'static str = CREDENTIALS_FILE;

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, StateError> {
        let path = self.root.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StateError::File {
                path: path.display().to_string(),
                action: "read",
                source,
            }),
        }
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), StateError> {
        self.ensure_directory()?;
        let path = self.root.join(name);
        let mut body = serde_json::to_vec_pretty(value).map_err(|error| StateError::Malformed {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        body.push(b'\n');
        let mut temporary =
            tempfile::NamedTempFile::new_in(&self.root).map_err(|source| StateError::File {
                path: path.display().to_string(),
                action: "staged",
                source,
            })?;
        restrict_to_owner(temporary.path(), 0o600)?;
        temporary
            .write_all(&body)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| StateError::File {
                path: path.display().to_string(),
                action: "staged",
                source,
            })?;
        temporary.persist(&path).map_err(|error| StateError::File {
            path: path.display().to_string(),
            action: "replaced atomically",
            source: error.error,
        })?;
        sync_directory(&self.root)?;
        Ok(())
    }

    fn ensure_directory(&self) -> Result<(), StateError> {
        std::fs::create_dir_all(&self.root).map_err(|source| StateError::File {
            path: self.root.display().to_string(),
            action: "created",
            source,
        })?;
        restrict_to_owner(&self.root, 0o700)
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32) -> Result<(), StateError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|source| {
        StateError::File {
            path: path.display().to_string(),
            action: "secured",
            source,
        }
    })
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path, _mode: u32) -> Result<(), StateError> {
    // Windows inherits the user profile's own ACL, which is already private.
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), StateError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| StateError::File {
            path: path.display().to_string(),
            action: "synchronized",
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), StateError> {
    // Atomic replacement is still provided by `tempfile`; directory handles
    // are not generally openable for an explicit metadata sync on Windows.
    Ok(())
}

#[cfg(test)]
mod tests {
    use briefcase_client::AccountKind;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::{
        Configuration, Credentials, Profile, SignInMethod, StateDirectory, StateError,
        StoredAccount, StoredSession,
    };

    pub(crate) fn session(expires_in: Duration) -> StoredSession {
        StoredSession {
            api_url: "https://briefcase.example/api/v1/".to_owned(),
            accounts_url: "https://accounts.example".to_owned(),
            app_id: "briefcase".to_owned(),
            account: StoredAccount {
                uuid: "zQo".to_owned(),
                id: "c:ada".to_owned(),
                kind: AccountKind::Carbon,
                display_name: Some("Ada".to_owned()),
                custodian: None,
            },
            access_token: "access-secret-value".to_owned(),
            refresh_token: "sar_refresh-secret-value".to_owned(),
            expires_at: OffsetDateTime::now_utc() + expires_in,
            refresh_expires_at: None,
            scope: vec!["profile".to_owned()],
            signed_in_at: OffsetDateTime::now_utc(),
            method: SignInMethod::Device,
        }
    }

    #[test]
    fn nothing_saved_yet_reads_as_empty_rather_than_failing() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path().join("state"));

        assert!(state.configuration().unwrap().profiles.is_empty());
        assert!(state.credentials().unwrap().sessions.is_empty());
        // Reading never creates the directory.
        assert!(!directory.path().join("state").exists());
    }

    #[test]
    fn a_saved_profile_and_sign_in_come_back() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path());

        let mut configuration = Configuration {
            current_profile: "work".to_owned(),
            ..Configuration::default()
        };
        configuration.profiles.insert(
            "work".to_owned(),
            Profile {
                url: "https://briefcase.example/api/v1/".to_owned(),
                accounts_url: Some("https://accounts.example".to_owned()),
            },
        );
        state.save_configuration(&configuration).unwrap();
        let mut credentials = Credentials::default();
        credentials.set_session("work", session(Duration::minutes(30)));
        state.save_credentials(&credentials).unwrap();

        let read = state.configuration().unwrap();
        assert_eq!(read.current_profile, "work");
        let credentials = state.credentials().unwrap();
        assert_eq!(credentials.format, 2);
        assert_eq!(credentials.session("work").unwrap().account.uuid, "zQo");
    }

    #[test]
    fn configuration_from_before_silicon_accounts_keeps_its_profiles_and_drops_the_organization() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("config.json"),
            r#"{"auto_update":false,"telemetry":false,"current_profile":"work",
                "profiles":{"work":{"url":"https://briefcase.example/api/v1/","org":"tos"}}}"#,
        )
        .unwrap();
        let state = StateDirectory::at(directory.path());
        let configuration = state.configuration().unwrap();
        assert_eq!(
            configuration.profiles["work"].url,
            "https://briefcase.example/api/v1/"
        );
        state.save_configuration(&configuration).unwrap();
        let written = std::fs::read_to_string(directory.path().join("config.json")).unwrap();
        assert!(!written.contains("\"org\""), "{written}");
        assert!(!written.contains("auto_update"), "{written}");
        assert!(!written.contains("telemetry"), "{written}");
    }

    #[test]
    fn credentials_from_before_silicon_accounts_read_as_signed_out_and_are_replaced() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("credentials.json"),
            r#"{"tokens":{"old":"oat_legacy"},
                "sessions":{"work":{"access_token":"oat_x","refresh_token":"ort_y",
                  "expires_at":"2026-09-01T00:00:00Z",
                  "actor":{"principal_id":"01a067ce-7f19-7790-820a-0be6b3d4f803","type":"carbon","public_id":"c:saket"},
                  "org_id":"tos"}},
                "testing_environment_keys":{}}"#,
        )
        .unwrap();
        let state = StateDirectory::at(directory.path());
        let credentials = state.credentials().unwrap();
        assert!(credentials.sessions.is_empty());
        let legacy = credentials.legacy.clone().unwrap();
        assert_eq!(legacy.profiles["work"].as_deref(), Some("c:saket"));
        assert_eq!(legacy.profiles["old"], None);

        state.save_credentials(&credentials).unwrap();
        let written = std::fs::read_to_string(state.credentials_path()).unwrap();
        assert!(!written.contains("oat_"), "{written}");
        assert!(!written.contains("ort_"), "{written}");
        assert!(state.credentials().unwrap().legacy.is_none());
    }

    #[test]
    fn a_corrupt_or_future_state_file_says_which_one_and_can_be_set_aside() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("credentials.json"), b"{ not json").unwrap();
        let state = StateDirectory::at(directory.path());

        let error = state.credentials().expect_err("corrupt state must fail");
        assert!(matches!(error, StateError::Malformed { .. }));
        assert!(error.to_string().contains("credentials.json"));

        let aside = state
            .set_aside_unreadable(StateDirectory::CREDENTIALS)
            .unwrap()
            .unwrap();
        assert!(aside.exists());
        assert_eq!(std::fs::read(&aside).unwrap(), b"{ not json");
        assert!(state.credentials().unwrap().sessions.is_empty());

        std::fs::write(
            directory.path().join("credentials.json"),
            br#"{"format":9}"#,
        )
        .unwrap();
        let error = state
            .credentials()
            .expect_err("a newer format must not be guessed at");
        assert!(error.to_string().contains("newer briefcase"));
    }

    #[test]
    fn sessions_refresh_inside_the_last_minute_and_debug_redacts_tokens() {
        assert!(!session(Duration::minutes(5)).needs_refresh());
        let expiring = session(Duration::seconds(30));
        assert!(expiring.needs_refresh());
        assert!(expiring.usable_for(Duration::seconds(10)));
        assert!(!session(Duration::seconds(5)).usable_for(Duration::seconds(10)));
        let rendered = format!("{expiring:?}");
        assert!(!rendered.contains("access-secret-value"));
        assert!(!rendered.contains("refresh-secret-value"));
    }

    #[test]
    fn an_ended_sign_in_is_remembered_until_the_next_one() {
        let mut credentials = Credentials::default();
        credentials.set_session("work", session(Duration::minutes(5)));
        credentials.end_session(
            "work",
            "sign_in_ended",
            "The refresh token was already used.",
        );
        assert!(credentials.session("work").is_none());
        assert_eq!(
            credentials.ended["work"].account_id.as_deref(),
            Some("c:ada")
        );
        credentials.set_session("work", session(Duration::minutes(5)));
        assert!(credentials.ended.is_empty());
    }

    #[test]
    fn a_crashed_mutation_reuses_its_key_and_ids_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path());
        let mut credentials = Credentials::default();
        let mut first = credentials.prepare_mutation("entry:move:one", "same-intent");
        first.resource_id = Some(Uuid::from_u128(9));
        first.destination_id = Some(Uuid::from_u128(10));
        credentials.set_pending_mutation("entry:move:one".to_owned(), first.clone());
        state.save_credentials(&credentials).unwrap();

        let mut restarted = state.credentials().unwrap();
        let replay = restarted.prepare_mutation("entry:move:one", "same-intent");
        assert_eq!(replay, first);
        let changed = restarted.prepare_mutation("entry:move:one", "another-intent");
        assert_ne!(changed.idempotency_key, first.idempotency_key);

        restarted.clear_pending_mutation("entry:move:one", &changed.idempotency_key);
        state.save_credentials(&restarted).unwrap();
        assert!(
            state
                .credentials()
                .unwrap()
                .pending_mutation("entry:move:one")
                .is_none()
        );
    }

    #[test]
    fn concurrent_credential_updates_serialize_their_read_modify_write() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path());
        state.save_credentials(&Credentials::default()).unwrap();
        let first_lock = state.lock_credentials().unwrap();
        let other_state = state.clone();
        let (attempting, attempted) = std::sync::mpsc::channel();
        let (finished, completion) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            attempting.send(()).unwrap();
            let _lock = other_state.lock_credentials().unwrap();
            let mut credentials = other_state.credentials().unwrap();
            credentials.set_session("second", session(Duration::minutes(5)));
            other_state.save_credentials(&credentials).unwrap();
            finished.send(()).unwrap();
        });
        attempted.recv().unwrap();

        let mut credentials = state.credentials().unwrap();
        credentials.set_session("first", session(Duration::minutes(5)));
        state.save_credentials(&credentials).unwrap();
        assert!(
            completion
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "the second writer must wait for the first credential lock"
        );
        drop(first_lock);
        completion
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        writer.join().unwrap();

        let credentials = state.credentials().unwrap();
        assert!(credentials.session("first").is_some());
        assert!(credentials.session("second").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_sign_in_file_and_directory_are_readable_only_by_their_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path().join("state"));
        state.save_credentials(&Credentials::default()).unwrap();

        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state.credentials_path()), 0o600);
        assert_eq!(mode(state.path()), 0o700);
        let _lock = state.lock_credentials().unwrap();
        assert_eq!(mode(&state.path().join("credentials.lock")), 0o600);
    }
}
