//! Private, durable state for the single gateway process. A lifetime lock prevents
//! two containers from rotating the same stored IAM refresh credentials.
use anyhow::{Context as _, Result};
use fs2::FileExt as _;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) struct Storage {
    directory: PathBuf,
    _lock: File,
    pub(crate) salt: String,
}

impl Storage {
    pub(crate) fn open(directory: &Path, upstream: &str, origin: &str) -> Result<Arc<Self>> {
        if directory.is_symlink() {
            anyhow::bail!("session directory must not be a symlink");
        }
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        anyhow::ensure!(
            !directory.join("owner.lock").is_symlink(),
            "invalid session lock"
        );
        let lock = options.open(directory.join("owner.lock"))?;
        lock.try_lock_exclusive()
            .context("another gateway owns the session directory")?;
        let mut storage = Self {
            directory: directory.to_owned(),
            _lock: lock,
            salt: String::new(),
        };
        let metadata: Option<serde_json::Value> = storage.read("metadata")?;
        storage.salt = if let Some(metadata) = metadata {
            anyhow::ensure!(
                metadata["upstream"] == upstream && metadata["origin"] == origin,
                "session directory belongs to a different API or browser origin"
            );
            metadata["salt"]
                .as_str()
                .filter(|value| value.len() >= 64)
                .context("invalid session storage identity")?
                .to_owned()
        } else {
            let salt = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            storage.write(
                "metadata",
                &serde_json::json!({"upstream":upstream,"origin":origin,"salt":salt}),
            )?;
            salt
        };
        Ok(Arc::new(storage))
    }

    pub(crate) fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.directory.join(format!("{name}.json"));
        anyhow::ensure!(!path.is_symlink(), "invalid session state path");
        match fs::read(path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid saved session state")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn write(&self, name: &str, value: &impl Serialize) -> Result<()> {
        let path = self.directory.join(format!("{name}.json"));
        anyhow::ensure!(!path.is_symlink(), "invalid session state path");
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        temporary.write_all(&serde_json::to_vec(value)?)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    pub(crate) fn identifiers(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && id.len() == 64
                && id.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                ids.push(id.to_owned());
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_private_durable_and_bound_to_one_gateway() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("sessions");
        let storage =
            Storage::open(&directory, "https://api.example", "https://app.example").unwrap();
        let salt = storage.salt.clone();
        assert!(Storage::open(&directory, "https://api.example", "https://app.example").is_err());
        storage
            .write("fixture", &serde_json::json!({"refresh_token":"private"}))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(directory.join("fixture.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        drop(storage);
        assert!(Storage::open(&directory, "https://other.example", "https://app.example").is_err());
        let reopened =
            Storage::open(&directory, "https://api.example", "https://app.example").unwrap();
        assert_eq!(reopened.salt, salt);
        assert_eq!(
            reopened
                .read::<serde_json::Value>("fixture")
                .unwrap()
                .unwrap()["refresh_token"],
            "private"
        );
    }
}
