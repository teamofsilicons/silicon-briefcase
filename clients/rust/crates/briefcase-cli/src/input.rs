//! Reading secrets — short-lived tokens, proofs, capabilities — without
//! echoing them.

use std::io::{BufRead as _, IsTerminal as _, Read as _};

use crate::error::{CliError, Result};

const MAXIMUM_SECRET_BYTES: u64 = 64 * 1024;

/// Asks for a secret at a hidden prompt, or reads one line from standard input
/// when it is not a terminal.
///
/// # Errors
///
/// Returns an I/O error when the terminal or standard input cannot be read, or
/// a usage error for input over 64 KiB.
pub fn prompt_secret(label: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        let secret = rpassword::prompt_password(label).map_err(|source| CliError::Io {
            path: "terminal".to_owned(),
            source,
        })?;
        return bounded_secret(&secret);
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(MAXIMUM_SECRET_BYTES + 1)
        .read_line(&mut line)
        .map_err(|source| CliError::Io {
            path: "standard input".to_owned(),
            source,
        })?;
    bounded_secret(&line)
}

/// Reads a secret from the whole of standard input.
///
/// # Errors
///
/// As [`prompt_secret`].
pub fn read_secret_stdin() -> Result<String> {
    let mut buffer = String::new();
    std::io::stdin()
        .take(MAXIMUM_SECRET_BYTES + 1)
        .read_to_string(&mut buffer)
        .map_err(|source| CliError::Io {
            path: "standard input".to_owned(),
            source,
        })?;
    bounded_secret(&buffer)
}

fn bounded_secret(value: &str) -> Result<String> {
    if value.len() as u64 > MAXIMUM_SECRET_BYTES {
        return Err(CliError::usage("secret input is limited to 64 KiB"));
    }
    Ok(value.trim().to_owned())
}
