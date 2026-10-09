//! A process-wide cache of the Silicon Accounts signing keys.
//!
//! Access tokens are verified locally against this set. A token naming a key
//! the cache does not hold triggers one refetch (keys rotate), but refetches
//! are spaced by [`UNKNOWN_KEY_REFETCH_INTERVAL`] so a flood of forged `kid`s
//! cannot turn into a flood of requests to Silicon Accounts. The set is also
//! refreshed after [`MAX_AGE`] so a withdrawn key stops verifying.

use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use silicon_accounts_client::{AccountsClient, Jwks};
use tokio::sync::Mutex;

/// Minimum spacing between refetches caused by an unknown `kid`.
pub const UNKNOWN_KEY_REFETCH_INTERVAL: Duration = Duration::from_secs(30);
/// Age after which the next verification refreshes the key set first.
pub const MAX_AGE: Duration = Duration::from_secs(3_600);

#[derive(Clone)]
struct Cached {
    jwks: Arc<Jwks>,
    fetched_at: Instant,
}

/// Cached JWKS with single-flight, rate-limited refreshes.
#[derive(Default)]
pub struct JwksCache {
    current: RwLock<Option<Cached>>,
    refresh: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for JwksCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys = self
            .current
            .read()
            .ok()
            .and_then(|current| current.as_ref().map(|cached| cached.jwks.keys.len()));
        formatter
            .debug_struct("JwksCache")
            .field("keys", &keys)
            .finish_non_exhaustive()
    }
}

impl JwksCache {
    /// Seeds the cache with a key set (tests and warm starts).
    #[must_use]
    pub fn with_keys(jwks: Jwks) -> Self {
        Self {
            current: RwLock::new(Some(Cached {
                jwks: Arc::new(jwks),
                fetched_at: Instant::now(),
            })),
            refresh: Mutex::new(None),
        }
    }

    /// Returns the cached key set, fetching it first when it is missing or
    /// older than [`MAX_AGE`]. A failed refresh of a stale set keeps serving
    /// the stale set; only a missing set is an error.
    ///
    /// # Errors
    ///
    /// Returns the client error when no key set is cached and the fetch fails.
    pub async fn get(&self, client: &AccountsClient) -> silicon_accounts_client::Result<Arc<Jwks>> {
        if let Some(cached) = self.snapshot()
            && cached.fetched_at.elapsed() < MAX_AGE
        {
            return Ok(cached.jwks);
        }
        let mut last_attempt = self.refresh.lock().await;
        // Another request may have refreshed while this one waited, or a
        // refresh may have just failed: keep serving a stale set rather than
        // retrying on every request while Silicon Accounts is unreachable.
        if let Some(cached) = self.snapshot()
            && (cached.fetched_at.elapsed() < MAX_AGE
                || last_attempt.is_some_and(|at| at.elapsed() < UNKNOWN_KEY_REFETCH_INTERVAL))
        {
            return Ok(cached.jwks);
        }
        *last_attempt = Some(Instant::now());
        match client.jwks().await {
            Ok(jwks) => Ok(self.store(jwks)),
            Err(error) => match self.snapshot() {
                Some(stale) => {
                    tracing::warn!(
                        event = "accounts_jwks_refresh_failed",
                        error_code = error.code(),
                        "Silicon Accounts key refresh failed; still verifying with the cached keys"
                    );
                    Ok(stale.jwks)
                }
                None => Err(error),
            },
        }
    }

    /// Refetches the key set because a token named an unknown key, unless a
    /// refetch happened within [`UNKNOWN_KEY_REFETCH_INTERVAL`].
    ///
    /// Returns `None` when the refetch was skipped or failed; the caller then
    /// refuses the token as signed by an unknown key.
    pub async fn refetch_for_unknown_key(&self, client: &AccountsClient) -> Option<Arc<Jwks>> {
        let mut last_attempt = self.refresh.lock().await;
        if last_attempt.is_some_and(|at| at.elapsed() < UNKNOWN_KEY_REFETCH_INTERVAL) {
            return None;
        }
        *last_attempt = Some(Instant::now());
        match client.jwks().await {
            Ok(jwks) => Some(self.store(jwks)),
            Err(error) => {
                tracing::warn!(
                    event = "accounts_jwks_refetch_failed",
                    error_code = error.code(),
                    "Silicon Accounts key refetch for an unknown key id failed"
                );
                None
            }
        }
    }

    fn snapshot(&self) -> Option<Cached> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn store(&self, jwks: Jwks) -> Arc<Jwks> {
        let jwks = Arc::new(jwks);
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Cached {
            jwks: Arc::clone(&jwks),
            fetched_at: Instant::now(),
        });
        jwks
    }
}
