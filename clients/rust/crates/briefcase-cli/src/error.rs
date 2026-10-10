//! Everything that can stop a command, said exactly: what failed, why, and what
//! to do next, with the exit code it deserves.

use briefcase_client::accounts::SignInError;

use crate::state::StateError;

/// How a Carbon signs in; how a Silicon does.
pub const SIGN_IN: &str = "Sign in: `briefcase login` (Carbons) or \
    `silicon-accounts login --app briefcase -q | briefcase login --slt-stdin` (Silicons).";
/// The same, after a sign-in ended.
pub const SIGN_IN_AGAIN: &str = "Sign in again: `briefcase login` (Carbons) or \
    `silicon-accounts login --app briefcase -q | briefcase login --slt-stdin` (Silicons).";

/// Everything that can stop a command.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Briefcase refused, or could not be reached.
    #[error(transparent)]
    Client(#[from] briefcase_client::Error),
    /// Briefcase refused, and the CLI knows what that refusal means here.
    #[error("{source}")]
    Explained {
        /// The refusal itself, which still decides the exit code.
        #[source]
        source: Box<briefcase_client::Error>,
        /// What it means for this command, and what to do instead.
        hint: String,
    },
    /// Signing in (or refreshing, or signing out) at Silicon Accounts failed.
    #[error(transparent)]
    SignIn(#[from] SignInError),
    /// Local state could not be read or written.
    #[error(transparent)]
    State(#[from] StateError),
    /// The command as typed cannot be carried out.
    #[error("{0}")]
    Usage(String),
    /// The CLI refused before asking anyone: not signed in, a retired option…
    #[error("{message}")]
    Refused {
        /// Stable machine-readable code.
        code: String,
        /// What is wrong and why.
        message: String,
        /// What to do next.
        hint: Option<String>,
        /// The exit code (2: as typed it cannot work; 4: no usable sign-in).
        exit: u8,
    },
    /// The storage probe completed, but did not activate the proposed bucket.
    #[error("storage validation failed; the previous configuration remains selected")]
    StorageValidationFailed,
    /// Exit with this code after everything was already said (no message).
    #[error("exit {0}")]
    Exit(u8),
    /// A local file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file involved.
        path: String,
        /// What went wrong.
        #[source]
        source: std::io::Error,
    },
}

/// The result of a CLI step.
pub type Result<T> = std::result::Result<T, CliError>;

impl CliError {
    /// A command that cannot work as typed.
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// A refusal the CLI makes itself.
    pub fn refused(
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
        exit: u8,
    ) -> Self {
        Self::Refused {
            code: code.into(),
            message: message.into(),
            hint,
            exit,
        }
    }

    /// A retired option or command, with what replaced it (exit code 2).
    pub fn retired(what: &str, replacement: &str) -> Self {
        Self::refused(
            "retired",
            format!("{what} is retired."),
            Some(replacement.to_owned()),
            2,
        )
    }

    /// Attaches what a refusal means for this command, when `hint` knows,
    /// keeping the exit code the refusal itself deserves.
    pub fn explained(
        error: briefcase_client::Error,
        hint: impl FnOnce(&briefcase_client::Error) -> Option<String>,
    ) -> Self {
        match hint(&error) {
            Some(hint) => Self::Explained {
                source: Box::new(error),
                hint,
            },
            None => Self::Client(error),
        }
    }

    /// Returns the process exit code this failure deserves.
    ///
    /// - `1` something failed,
    /// - `2` the command as typed cannot be carried out,
    /// - `3` the entry was not found, or is not yours to see,
    /// - `4` no usable sign-in, or the action is not allowed.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Refused { exit, .. } | Self::Exit(exit) => i32::from(*exit),
            Self::SignIn(error) => match error {
                SignInError::Unreachable { .. }
                | SignInError::DeviceExpired { .. }
                | SignInError::Accounts { .. } => 1,
                SignInError::InvalidInput { .. } => 2,
                _ => 4,
            },
            _ => match self.refusal() {
                Some(error) if error.is_not_found() => 3,
                Some(error) if error.is_forbidden() || error.is_unauthenticated() => 4,
                _ => 1,
            },
        }
    }

    /// Briefcase's own answer, when the failure is one.
    fn refusal(&self) -> Option<&briefcase_client::Error> {
        match self {
            Self::Client(error) => Some(error),
            Self::Explained { source, .. } => Some(source),
            _ => None,
        }
    }

    /// The stable machine-readable code.
    #[must_use]
    pub fn code(&self) -> String {
        if let Some(code) = self.refusal().and_then(briefcase_client::Error::code) {
            return code.to_owned();
        }
        match self {
            Self::Client(briefcase_client::Error::Incompatible(_)) => {
                "incompatible_contract".to_owned()
            }
            Self::Client(briefcase_client::Error::Transport { .. }) => {
                "briefcase_unreachable".to_owned()
            }
            Self::Client(briefcase_client::Error::Protocol(_)) => "unreadable_response".to_owned(),
            Self::Client(briefcase_client::Error::Configuration(_)) => {
                "invalid_configuration".to_owned()
            }
            Self::Client(_) | Self::Explained { .. } | Self::Io { .. } => {
                "local_file_failed".to_owned()
            }
            Self::SignIn(error) => error.code().to_owned(),
            Self::State(StateError::Malformed { .. }) => "unreadable_state".to_owned(),
            Self::State(_) => "state_unavailable".to_owned(),
            Self::Usage(_) => "usage".to_owned(),
            Self::Refused { code, .. } => code.clone(),
            Self::StorageValidationFailed => "storage_validation_failed".to_owned(),
            Self::Exit(_) => "exit".to_owned(),
        }
    }

    /// What to do next, when there is something to say.
    #[must_use]
    pub fn hint(&self) -> Option<String> {
        match self {
            Self::Explained { hint, .. } => Some(hint.clone()),
            Self::Client(error) => client_hint(error),
            Self::SignIn(error) => error.hint(),
            Self::State(StateError::Malformed { .. }) => Some(
                "Run `briefcase login` to sign in again; the unreadable file is kept beside it as \
                 *.unreadable-<time>."
                    .to_owned(),
            ),
            Self::State(StateError::NoHome) => Some(
                "Set SILICON_HOME or HOME, or BRIEFCASE_HOME for the state directory itself."
                    .to_owned(),
            ),
            Self::Refused { hint, .. } => hint.clone(),
            Self::StorageValidationFailed => Some(
                "After fixing the bucket or role, run it again with a new --operation-id."
                    .to_owned(),
            ),
            Self::State(_) | Self::Usage(_) | Self::Io { .. } | Self::Exit(_) => None,
        }
    }

    /// The error as JSON, for `--json`: `{"error":{"code","message","hint"?,"status"?,"request_id"?}}`.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let mut error = serde_json::json!({
            "code": self.code(),
            "message": self.to_string(),
        });
        if let Some(hint) = self.hint() {
            error["hint"] = hint.into();
        }
        if let Some(briefcase_client::Error::Api(api)) = self.refusal() {
            error["status"] = api.status.into();
            if let Some(request_id) = &api.request_id {
                error["request_id"] = request_id.clone().into();
            }
        }
        serde_json::json!({ "error": error })
    }
}

/// Hints for refusals Briefcase answers without one.
fn client_hint(error: &briefcase_client::Error) -> Option<String> {
    if let Some(hint) = error.hint() {
        return Some(hint.to_owned());
    }
    if let briefcase_client::Error::Incompatible(_) = error {
        return Some(
            "Use matching CLI and server versions: check `briefcase version` and the deployment's /api/version."
                .to_owned(),
        );
    }
    if let briefcase_client::Error::Transport { .. } = error {
        return Some(
            "Check the network and the API URL (`briefcase status`), then try again.".to_owned(),
        );
    }
    let hint = match error.code()? {
        "token_expired"
        | "token_revoked"
        | "token_invalid"
        | "token_malformed"
        | "token_wrong_audience"
        | "token_wrong_issuer"
        | "unauthenticated"
        | "token_missing" => SIGN_IN_AGAIN,
        "account_deleted" => {
            "This account was deleted at Silicon Accounts and can no longer use Briefcase."
        }
        "account_access_removed" => {
            "This account removed Briefcase at Silicon Accounts; signing in again restores it."
        }
        "not_found" => {
            "Briefcase answers the same for an entry that does not exist and one that is not shared with you; \
             check the path and its drive (@c:<id>/… for another account's drive)."
        }
        "cross_drive_move" => {
            "Entries move only within their drive; copy it with `briefcase get` and `briefcase put` instead."
        }
        "silicon_not_open" => {
            "A Silicon accepts shares only from its custodian, the custodian's other Silicons, and accounts it \
             allowed: the Silicon or its custodian can run `briefcase allow add <your id> --for <silicon>`."
        }
        "unknown_account" => {
            "Check the id (c:<handle> or si:<handle>); ids can change, so ask the account for its current one."
        }
        "account_inactive" => {
            "That account is deleted, or its Silicon still waits for its custodian."
        }
        "not_silicons_custodian" => {
            "Only the Silicon itself and its custodian manage who may share with it."
        }
        "forbidden" => {
            "Only an entry's creator, its drive's owner and their custodians may do this; others need a share."
        }
        "daily_upload_limit_exhausted" => {
            "The drive's daily upload allowance is spent; it returns at midnight UTC (see `briefcase usage`)."
        }
        "storage_limit_exhausted" => {
            "The drive's storage is full; empty its bin or delete files (see `briefcase usage`)."
        }
        "protected_folder" => {
            "The top of a drive, `apps/` and `apps/<app>/` are structure: they cannot be renamed, moved, deleted, shared or linked."
        }
        _ => return None,
    };
    Some(hint.to_owned())
}

#[cfg(test)]
mod tests {
    use super::CliError;

    fn refusal(status: u16, code: &str) -> briefcase_client::Error {
        briefcase_client::Error::from(briefcase_client::ApiError {
            status,
            code: code.to_owned(),
            message: "Not allowed".to_owned(),
            hint: None,
            request_id: Some("req-1".to_owned()),
            retry_after: None,
            unsatisfied_range_length: None,
        })
    }

    #[test]
    fn exit_codes_tell_a_script_what_happened() {
        assert_eq!(CliError::usage("nope").exit_code(), 2);
        assert_eq!(CliError::Client(refusal(404, "not_found")).exit_code(), 3);
        assert_eq!(CliError::Client(refusal(403, "forbidden")).exit_code(), 4);
        assert_eq!(
            CliError::Client(refusal(401, "token_expired")).exit_code(),
            4
        );
        assert_eq!(CliError::Client(refusal(409, "conflict")).exit_code(), 1);
        assert_eq!(CliError::retired("--org", "x").exit_code(), 2);
    }

    #[test]
    fn an_explained_refusal_keeps_the_exit_code_of_the_refusal() {
        let explained = CliError::explained(refusal(403, "forbidden"), |_| {
            Some("ask the creator".to_owned())
        });
        assert_eq!(explained.exit_code(), 4);
        assert_eq!(explained.hint().as_deref(), Some("ask the creator"));
        assert!(matches!(
            CliError::explained(refusal(409, "conflict"), |_| None),
            CliError::Client(_)
        ));
    }

    #[test]
    fn json_errors_carry_code_message_hint_status_and_request() {
        let json = CliError::Client(refusal(401, "token_revoked")).to_json();
        assert_eq!(json["error"]["code"], "token_revoked");
        assert_eq!(json["error"]["status"], 401);
        assert_eq!(json["error"]["request_id"], "req-1");
        assert!(
            json["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("briefcase login")
        );
    }
}
