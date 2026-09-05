//! What the CLI remembers between runs.
//!
//! The package underneath is stateless; this is the part that is not. Profiles
//! and rotating sessions live under `~/.briefcase/`; sessions and environment
//! root keys are in a file only the owner can read, so a shell does not need to
//! carry long-lived credentials in its history or environment.

use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use briefcase_client::{EnvironmentKey, IdempotencyKey, SessionActor, SessionTokens};
use fs2::FileExt as _;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Directory name under the user's home.
const STATE_DIRECTORY: &str = ".briefcase";
/// Profiles and their deployments.
const CONFIG_FILE: &str = "config.json";
/// Tokens, readable only by their owner.
const CREDENTIALS_FILE: &str = "credentials.json";
/// Cross-process lock protecting state read/modify/write transactions.
const CREDENTIALS_LOCK_FILE: &str = "credentials.lock";
/// Non-secret updater throttle state.
const UPDATE_FILE: &str = "update.json";

/// Something the CLI could not read or write locally.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// The home directory could not be determined.
    #[error("no home directory: set HOME, or pass --url, --org and --token")]
    NoHome,
    /// A state file could not be read or written.
    #[error("{path} could not be {action}")]
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

/// One saved deployment and organization.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Profile {
    /// Versioned API base URL.
    pub url: String,
    /// Organization every request is scoped to.
    pub org: String,
}

/// Every saved profile, and which one is current.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Configuration {
    /// Whether the installed CLI checks crates.io once per day.
    #[serde(default = "enabled")]
    pub auto_update: bool,
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
            auto_update: true,
            current_profile: default_profile_name(),
            profiles: BTreeMap::new(),
        }
    }
}

fn enabled() -> bool {
    true
}

/// Non-secret throttle state for the daily CLI updater.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct UpdateState {
    /// Compiled CLI version whose check was most recently attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_version: Option<String>,
    /// Time of the most recent check attempt.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub checked_at: Option<OffsetDateTime>,
}

fn default_profile_name() -> String {
    "default".to_owned()
}

/// Legacy tokens, rotating sessions, and testing root keys.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Credentials {
    /// Legacy access-only tokens, retained so old installations keep working.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    /// Production sessions, keyed by profile.
    #[serde(default)]
    pub sessions: BTreeMap<String, StoredSession>,
    /// Test sessions, separated by profile and public environment UUID.
    #[serde(default)]
    pub test_sessions: BTreeMap<String, BTreeMap<Uuid, StoredSession>>,
    /// Briefcase root keys, separated by profile and addressed by public UUID.
    #[serde(default)]
    pub testing_environment_keys: BTreeMap<String, BTreeMap<Uuid, EnvironmentKey>>,
    /// Canonical deployment/organization binding for production credentials.
    #[serde(default)]
    pub production_credential_scopes: BTreeMap<String, CredentialScope>,
    /// Canonical deployment/organization binding for each test root/session.
    #[serde(default)]
    pub testing_environment_scopes: BTreeMap<String, BTreeMap<Uuid, CredentialScope>>,
    /// Crash-recovery identity for one-time and generated-secret mutations.
    #[serde(default)]
    pub pending_mutations: BTreeMap<String, PendingMutation>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("legacy_token_profiles", &self.tokens.keys())
            .field("sessions", &self.sessions)
            .field("test_sessions", &self.test_sessions)
            .field("testing_environment_keys", &self.testing_environment_keys)
            .field(
                "production_credential_scope_profiles",
                &self.production_credential_scopes.keys(),
            )
            .field(
                "testing_environment_scope_profiles",
                &self.testing_environment_scopes.keys(),
            )
            .field("pending_mutation_scopes", &self.pending_mutations.keys())
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
    /// Optimistic version captured before an environment metadata update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// Stable entry resolved before a path-addressed mutation was attempted.
    ///
    /// Keeping this lets a retry find the original entry even when the first
    /// attempt already renamed or moved it before the CLI lost its response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<Uuid>,
    /// Stable destination folder resolved before a path-addressed move.
    ///
    /// A retry must send the original parent UUID even if that folder's path
    /// changed after the first attempt committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_id: Option<Uuid>,
}

/// Where a stored bearer or testing root is allowed to be sent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CredentialScope {
    /// Canonical scheme, host, and port of the Briefcase deployment.
    pub deployment_origin: String,
    /// Organization the credential was acquired for.
    pub organization: String,
}

impl Credentials {
    /// Returns the persisted binding for a production or testing credential.
    #[must_use]
    pub fn credential_scope(
        &self,
        profile: &str,
        environment: Option<Uuid>,
    ) -> Option<&CredentialScope> {
        match environment {
            Some(id) => self.testing_environment_scopes.get(profile)?.get(&id),
            None => self.production_credential_scopes.get(profile),
        }
    }

    /// Binds a production session or test root/session to one destination.
    pub fn set_credential_scope(
        &mut self,
        profile: &str,
        environment: Option<Uuid>,
        scope: CredentialScope,
    ) {
        match environment {
            Some(id) => {
                self.testing_environment_scopes
                    .entry(profile.to_owned())
                    .or_default()
                    .insert(id, scope);
            }
            None => {
                self.production_credential_scopes
                    .insert(profile.to_owned(), scope);
            }
        }
    }

    /// Adds destination bindings to credentials written by an older CLI.
    ///
    /// Call this with the profile's old saved destination before changing that
    /// profile, so migration can never reinterpret a credential at a new host.
    pub fn bind_legacy_profile_scope(&mut self, profile: &str, scope: &CredentialScope) {
        if self.credential_scope(profile, None).is_none()
            && (self.sessions.contains_key(profile) || self.tokens.contains_key(profile))
        {
            self.set_credential_scope(profile, None, scope.clone());
        }
        let mut environments = self
            .testing_environment_keys
            .get(profile)
            .into_iter()
            .flat_map(BTreeMap::keys)
            .copied()
            .collect::<Vec<_>>();
        environments.extend(
            self.test_sessions
                .get(profile)
                .into_iter()
                .flat_map(BTreeMap::keys)
                .copied(),
        );
        environments.sort_unstable();
        environments.dedup();
        for environment in environments {
            if self.credential_scope(profile, Some(environment)).is_none() {
                self.set_credential_scope(profile, Some(environment), scope.clone());
            }
        }
    }
    /// Finds the rotating session for exactly one production or test plane.
    #[must_use]
    pub fn session(&self, profile: &str, environment: Option<Uuid>) -> Option<&StoredSession> {
        match environment {
            Some(id) => self.test_sessions.get(profile)?.get(&id),
            None => self.sessions.get(profile),
        }
    }

    /// Replaces the rotating session for exactly one production or test plane.
    pub fn set_session(
        &mut self,
        profile: &str,
        environment: Option<Uuid>,
        session: StoredSession,
    ) {
        match environment {
            Some(id) => {
                self.test_sessions
                    .entry(profile.to_owned())
                    .or_default()
                    .insert(id, session);
            }
            None => {
                self.sessions.insert(profile.to_owned(), session);
            }
        }
    }

    /// Removes only the production or test session selected by this invocation.
    pub fn remove_session(&mut self, profile: &str, environment: Option<Uuid>) -> bool {
        match environment {
            Some(id) => self
                .test_sessions
                .get_mut(profile)
                .and_then(|sessions| sessions.remove(&id))
                .is_some(),
            None => self.sessions.remove(profile).is_some(),
        }
    }

    /// Returns the Briefcase root key behind a public test-environment UUID.
    #[must_use]
    pub fn testing_environment_key(
        &self,
        profile: &str,
        environment: Uuid,
    ) -> Option<&EnvironmentKey> {
        self.testing_environment_keys
            .get(profile)?
            .get(&environment)
    }

    /// Stores or replaces a Briefcase root key without putting it in config.
    pub fn set_testing_environment_key(
        &mut self,
        profile: &str,
        environment: Uuid,
        key: EnvironmentKey,
    ) {
        self.testing_environment_keys
            .entry(profile.to_owned())
            .or_default()
            .insert(environment, key);
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
    pub fn prepare_mutation(
        &mut self,
        scope: &str,
        request_fingerprint: &str,
        expected_version: Option<i64>,
    ) -> PendingMutation {
        if let Some(pending) = self.pending_mutation(scope)
            && pending.request_fingerprint == request_fingerprint
        {
            let mut pending = pending.clone();
            if pending.expected_version.is_none() && expected_version.is_some() {
                pending.expected_version = expected_version;
                self.set_pending_mutation(scope.to_owned(), pending.clone());
            }
            return pending;
        }
        let pending = PendingMutation {
            idempotency_key: IdempotencyKey::random().as_str().to_owned(),
            request_fingerprint: request_fingerprint.to_owned(),
            expected_version,
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
}

/// One rotating Briefcase session obtained from an IAM short-lived token.
#[derive(Clone, Deserialize, Serialize)]
pub struct StoredSession {
    /// Short-lived bearer sent to normal Briefcase operations.
    pub access_token: String,
    /// Single-use refresh token rotated before expiry.
    pub refresh_token: String,
    /// Time at which the access token expires.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    /// IAM principal represented by the session.
    pub actor: SessionActor,
    /// Organization selected when the SLT was exchanged.
    pub org_id: Option<String>,
    /// Retry identity persisted before an in-flight refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_idempotency_key: Option<String>,
}

impl StoredSession {
    /// Converts a wire token response into durable state.
    #[must_use]
    pub fn from_tokens(tokens: &SessionTokens) -> Self {
        Self {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: OffsetDateTime::now_utc()
                + time::Duration::seconds(i64::try_from(tokens.expires_in).unwrap_or(i64::MAX)),
            actor: tokens.actor.clone(),
            org_id: tokens.org_id.clone(),
            refresh_idempotency_key: None,
        }
    }

    /// Whether renewal should happen before the next request starts.
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        self.expires_at <= OffsetDateTime::now_utc() + time::Duration::minutes(1)
    }
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredSession")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("actor", &self.actor)
            .field("org_id", &self.org_id)
            .field(
                "refresh_idempotency_key",
                &self.refresh_idempotency_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
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

impl StateDirectory {
    /// Locates the state directory, honoring `BRIEFCASE_HOME` when set.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::NoHome`] when neither `BRIEFCASE_HOME` nor `HOME`
    /// names a directory.
    pub fn locate() -> Result<Self, StateError> {
        if let Ok(explicit) = std::env::var("BRIEFCASE_HOME")
            && !explicit.trim().is_empty()
        {
            return Ok(Self {
                root: PathBuf::from(explicit),
            });
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(StateError::NoHome)?;
        Ok(Self {
            root: home.join(STATE_DIRECTORY),
        })
    }

    /// Uses an explicit directory, which is what the tests do.
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

    /// Reads the saved profiles, or an empty set when nothing is saved yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn configuration(&self) -> Result<Configuration, StateError> {
        self.read_json(CONFIG_FILE)
    }

    /// Reads the saved tokens, or an empty set when nothing is saved yet.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read or parsed.
    pub fn credentials(&self) -> Result<Credentials, StateError> {
        self.read_json(CREDENTIALS_FILE)
    }

    /// Serializes a state read/modify/write sequence across CLI processes.
    ///
    /// Callers performing a rotating-token exchange deliberately keep the
    /// returned guard alive for the network request. That prevents two CLI
    /// processes from presenting the same one-time refresh credential.
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

    /// Reads the updater throttle state, or an empty value when absent.
    pub fn update_state(&self) -> Result<UpdateState, StateError> {
        self.read_json(UPDATE_FILE)
    }

    /// Writes the profiles.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save_configuration(&self, configuration: &Configuration) -> Result<(), StateError> {
        self.write_json(CONFIG_FILE, configuration, 0o600)
    }

    /// Writes the tokens, readable only by their owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or file cannot be written.
    pub fn save_credentials(&self, credentials: &Credentials) -> Result<(), StateError> {
        self.write_json(CREDENTIALS_FILE, credentials, 0o600)
    }

    /// Records a successful crates.io check without storing credentials.
    pub fn save_update_state(&self, state: &UpdateState) -> Result<(), StateError> {
        self.write_json(UPDATE_FILE, state, 0o600)
    }

    /// Returns where the token file lives, so a message can name it.
    #[must_use]
    pub fn credentials_path(&self) -> PathBuf {
        self.root.join(CREDENTIALS_FILE)
    }

    fn read_json<T: Default + serde::de::DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<T, StateError> {
        let path = self.root.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| StateError::Malformed {
                path: path.display().to_string(),
                reason: error.to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(source) => Err(StateError::File {
                path: path.display().to_string(),
                action: "read",
                source,
            }),
        }
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T, mode: u32) -> Result<(), StateError> {
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
        restrict_to_owner(temporary.path(), mode)?;
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
    use time::Duration;
    use uuid::Uuid;

    use super::{Configuration, Credentials, Profile, StateDirectory, StoredSession};

    fn session(expires_in: Duration) -> StoredSession {
        StoredSession {
            access_token: "access-secret-value".to_owned(),
            refresh_token: "refresh-secret-value".to_owned(),
            expires_at: time::OffsetDateTime::now_utc() + expires_in,
            actor: briefcase_client::SessionActor {
                principal_id: Uuid::from_u128(42),
                actor_type: briefcase_client::ActorType::Carbon,
                public_id: "cos:tester".to_owned(),
            },
            org_id: Some("tos".to_owned()),
            refresh_idempotency_key: None,
        }
    }

    #[test]
    fn nothing_saved_yet_reads_as_empty_rather_than_failing() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path().join("state"));

        assert!(state.configuration().unwrap().profiles.is_empty());
        assert!(state.credentials().unwrap().tokens.is_empty());
        assert!(state.configuration().unwrap().auto_update);
    }

    #[test]
    fn old_configuration_files_gain_default_on_updates() {
        let configuration: Configuration =
            serde_json::from_str(r#"{"current_profile":"default","profiles":{}}"#).unwrap();
        assert!(configuration.auto_update);
    }

    #[test]
    fn sessions_and_root_keys_are_partitioned_by_test_plane() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let mut credentials = Credentials::default();
        credentials.set_session("work", None, session(Duration::minutes(4)));
        credentials.set_session("work", Some(first), session(Duration::minutes(5)));
        credentials.set_session("work", Some(second), session(Duration::minutes(6)));
        credentials.set_testing_environment_key(
            "work",
            first,
            briefcase_client::EnvironmentKey::new("a".repeat(32)).unwrap(),
        );

        assert!(credentials.session("work", None).is_some());
        assert!(credentials.session("work", Some(first)).is_some());
        assert!(credentials.session("work", Some(second)).is_some());
        assert!(credentials.testing_environment_key("work", first).is_some());
        assert!(
            credentials
                .testing_environment_key("other", first)
                .is_none()
        );
    }

    #[test]
    fn access_sessions_refresh_before_expiry_and_debug_redacts_tokens() {
        let fresh = session(Duration::minutes(5));
        assert!(!fresh.needs_refresh());
        let expiring = session(Duration::seconds(20));
        assert!(expiring.needs_refresh());
        let rendered = format!("{expiring:?}");
        assert!(!rendered.contains("access-secret-value"));
        assert!(!rendered.contains("refresh-secret-value"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn a_saved_profile_and_token_come_back() {
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
                org: "tos".to_owned(),
            },
        );
        state.save_configuration(&configuration).unwrap();

        let mut credentials = Credentials::default();
        credentials
            .tokens
            .insert("work".to_owned(), "secret".to_owned());
        state.save_credentials(&credentials).unwrap();

        let read = state.configuration().unwrap();
        assert_eq!(read.current_profile, "work");
        assert_eq!(read.profiles["work"].org, "tos");
        assert_eq!(state.credentials().unwrap().tokens["work"], "secret");
    }

    #[test]
    fn a_crashed_mutation_reuses_its_key_and_expected_version_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path());
        let mut credentials = Credentials::default();
        let mut first = credentials.prepare_mutation("env:update:one", "same-intent", Some(7));
        first.resource_id = Some(Uuid::from_u128(9));
        first.destination_id = Some(Uuid::from_u128(10));
        credentials.set_pending_mutation("env:update:one".to_owned(), first.clone());
        state.save_credentials(&credentials).unwrap();

        let mut restarted = state.credentials().unwrap();
        let replay = restarted.prepare_mutation("env:update:one", "same-intent", None);
        assert_eq!(replay, first);
        assert_eq!(replay.expected_version, Some(7));
        assert_eq!(replay.resource_id, Some(Uuid::from_u128(9)));
        assert_eq!(replay.destination_id, Some(Uuid::from_u128(10)));

        restarted.clear_pending_mutation("env:update:one", &replay.idempotency_key);
        state.save_credentials(&restarted).unwrap();
        assert!(
            state
                .credentials()
                .unwrap()
                .pending_mutation("env:update:one")
                .is_none()
        );
    }

    #[test]
    fn changed_intent_never_reuses_an_idempotency_key() {
        let mut credentials = Credentials::default();
        let first = credentials.prepare_mutation("login:work", "first-slt", None);
        let second = credentials.prepare_mutation("login:work", "second-slt", None);

        assert_ne!(first.idempotency_key, second.idempotency_key);
        assert_eq!(second.request_fingerprint, "second-slt");
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
            credentials
                .tokens
                .insert("second".to_owned(), "newer".to_owned());
            other_state.save_credentials(&credentials).unwrap();
            finished.send(()).unwrap();
        });
        attempted.recv().unwrap();

        let mut credentials = state.credentials().unwrap();
        credentials
            .tokens
            .insert("first".to_owned(), "older".to_owned());
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
        assert_eq!(credentials.tokens["first"], "older");
        assert_eq!(credentials.tokens["second"], "newer");
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let state = StateDirectory::at(directory.path());
        state.save_credentials(&Credentials::default()).unwrap();

        let mode = std::fs::metadata(state.credentials_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_corrupt_state_file_says_which_one() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("config.json"), b"{ not json").unwrap();
        let state = StateDirectory::at(directory.path());

        let error = state.configuration().expect_err("corrupt state must fail");
        assert!(error.to_string().contains("config.json"));
    }
}
