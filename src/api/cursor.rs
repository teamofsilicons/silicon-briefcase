//! Versioned opaque pagination cursors.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

const CURSOR_VERSION: u8 = 1;
const MAXIMUM_CURSOR_BYTES: usize = 2_048;

/// Stable keyset position for entry and bin pages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryCursor {
    /// Last timestamp returned by the previous page.
    pub timestamp: OffsetDateTime,
    /// Last UUID returned at that timestamp.
    pub entry_id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    version: u8,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
    entry_id: Uuid,
}

/// Cursor parsing or serialization failure.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CursorError {
    /// Encoded cursor exceeds the transport limit.
    #[error("cursor is too long")]
    TooLong,
    /// Cursor is not valid base64url JSON with the current schema.
    #[error("cursor is malformed")]
    Malformed,
    /// Cursor uses an unsupported schema version.
    #[error("cursor version is unsupported")]
    UnsupportedVersion,
}

/// Serializes a keyset position to base64url without padding.
///
/// # Errors
///
/// Returns [`CursorError::Malformed`] if the internal cursor envelope cannot
/// be serialized.
pub fn encode(cursor: EntryCursor) -> Result<String, CursorError> {
    let payload = serde_json::to_vec(&CursorEnvelope {
        version: CURSOR_VERSION,
        timestamp: cursor.timestamp,
        entry_id: cursor.entry_id,
    })
    .map_err(|_| CursorError::Malformed)?;
    Ok(URL_SAFE_NO_PAD.encode(payload))
}

/// Parses and validates an opaque keyset cursor.
///
/// # Errors
///
/// Returns a classified cursor error for oversized, malformed, or unsupported
/// input.
pub fn decode(value: &str) -> Result<EntryCursor, CursorError> {
    if value.len() > MAXIMUM_CURSOR_BYTES {
        return Err(CursorError::TooLong);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| CursorError::Malformed)?;
    let cursor =
        serde_json::from_slice::<CursorEnvelope>(&bytes).map_err(|_| CursorError::Malformed)?;
    if cursor.version != CURSOR_VERSION {
        return Err(CursorError::UnsupportedVersion);
    }
    Ok(EntryCursor {
        timestamp: cursor.timestamp,
        entry_id: cursor.entry_id,
    })
}

/// Validates the bounded base64url transport encoding without interpreting a
/// repository-owned cursor payload.
///
/// # Errors
///
/// Returns [`CursorError::TooLong`] for an oversized cursor and
/// [`CursorError::Malformed`] for invalid base64url input.
pub fn validate_opaque(value: &str) -> Result<(), CursorError> {
    if value.len() > MAXIMUM_CURSOR_BYTES {
        return Err(CursorError::TooLong);
    }
    URL_SAFE_NO_PAD
        .decode(value)
        .map(|_| ())
        .map_err(|_| CursorError::Malformed)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use uuid::Uuid;

    use super::{CursorError, EntryCursor, decode, encode};

    #[test]
    fn cursor_round_trips() -> anyhow::Result<()> {
        let cursor = EntryCursor {
            timestamp: datetime!(2026-08-31 12:00:00 UTC),
            entry_id: Uuid::now_v7(),
        };
        let encoded = encode(cursor)?;
        assert_eq!(decode(&encoded), Ok(cursor));
        Ok(())
    }

    #[test]
    fn arbitrary_input_is_rejected() {
        assert_eq!(decode("not/a/cursor"), Err(CursorError::Malformed));
    }
}
