//! Gateway-local admission accounting, independent of authoritative org quotas.
use crate::{Failure, Result};
use axum::http::StatusCode;
use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

pub(crate) const MAX_FILE_BYTES: u64 = 5 * 1024_u64.pow(4);

#[derive(Default)]
struct Usage {
    // Includes files waiting for the upstream response, not just incoming bodies.
    reserved: u64,
    unwritten: u64,
}

pub(crate) struct Storage {
    directory: tempfile::TempDir,
    budget: u64,
    free_reserve: u64,
    usage: Mutex<Usage>,
    pub(crate) deadline: std::time::Duration,
}

impl Storage {
    pub(crate) fn from_env() -> anyhow::Result<Arc<Self>> {
        let directory = match std::env::var_os("BRIEFCASE_WEB_UPLOAD_DIRECTORY") {
            Some(path) => tempfile::Builder::new()
                .prefix("briefcase-web-")
                .tempdir_in(path)?,
            None => tempfile::Builder::new()
                .prefix("briefcase-web-")
                .tempdir()?,
        };
        let number = |name: &str, default: u64| -> anyhow::Result<u64> {
            match std::env::var(name) {
                Ok(value) => Ok(value.parse::<u64>()?),
                Err(std::env::VarError::NotPresent) => Ok(default),
                Err(error) => Err(error.into()),
            }
        };
        let budget = number("BRIEFCASE_WEB_UPLOAD_BUDGET_BYTES", 4 * MAX_FILE_BYTES)?;
        let free_reserve = number("BRIEFCASE_WEB_UPLOAD_FREE_RESERVE_BYTES", 1024_u64.pow(3))?;
        let deadline = number("BRIEFCASE_WEB_UPLOAD_DEADLINE_SECONDS", 86400)?;
        anyhow::ensure!(
            budget > 0 && deadline > 0,
            "Upload budget and deadline must be positive"
        );
        Ok(Arc::new(Self {
            directory,
            budget,
            free_reserve,
            usage: Mutex::new(Usage::default()),
            deadline: std::time::Duration::from_secs(deadline),
        }))
    }

    pub(crate) fn directory(&self) -> &Path {
        self.directory.path()
    }

    fn usage(&self) -> MutexGuard<'_, Usage> {
        self.usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn reserve(self: &Arc<Self>, size: u64) -> Result<Reservation> {
        let mut reservation = Reservation {
            storage: self.clone(),
            reserved: 0,
            unwritten: 0,
        };
        reservation.grow(size)?;
        Ok(reservation)
    }
}

// Held until both file handles are closed and the private temporary file is
// removed. Drop also releases accounting on disconnect, timeout and SDK errors.
pub(crate) struct Reservation {
    storage: Arc<Storage>,
    reserved: u64,
    unwritten: u64,
}

impl Reservation {
    pub(crate) fn grow(&mut self, bytes: u64) -> Result<()> {
        let mut usage = self.storage.usage();
        let available =
            fs2::available_space(self.storage.directory()).map_err(|_| unavailable())?;
        if bytes > self.storage.budget.saturating_sub(usage.reserved)
            || bytes
                > available
                    .saturating_sub(self.storage.free_reserve)
                    .saturating_sub(usage.unwritten)
        {
            return Err(unavailable());
        }
        usage.reserved += bytes;
        usage.unwritten += bytes;
        self.reserved += bytes;
        self.unwritten += bytes;
        Ok(())
    }

    pub(crate) fn written(&mut self, bytes: u64) {
        // Call only after the write has completed: until then those bytes must
        // remain subtracted from free space for other concurrent admissions.
        self.storage.usage().unwritten -= bytes;
        self.unwritten -= bytes;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut usage = self.storage.usage();
        usage.reserved -= self.reserved;
        usage.unwritten -= self.unwritten;
    }
}

fn unavailable() -> Failure {
    Failure(StatusCode::INSUFFICIENT_STORAGE,
        "The browser gateway cannot reserve temporary storage for this upload. Retry later or use the Briefcase CLI/client.".into())
}
