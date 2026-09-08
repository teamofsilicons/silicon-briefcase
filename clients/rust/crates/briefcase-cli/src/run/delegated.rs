//! Local manifest preparation and one-shot delegated SDK calls.

use std::{
    fs::File,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
};

use briefcase_client::{
    ApplicationId, Client, ContentStream, UploadSource,
    delegated::{
        DelegatedCancelUpload, DelegatedCommitUpload, DelegatedCreateFolder, DelegatedListEntries,
        DelegatedManifest, DelegatedOperation as ManifestOperation, DelegatedReadFile,
        DelegatedReserveUpload, DelegatedTrashEntry, DelegatedUploadQuery, DelegatedUploadStatus,
        OboProof, UploadCapability,
    },
};
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

use crate::{
    cli::{AppRequestArgs, DelegatedOperation, GlobalArgs},
    render::Output,
};

use super::{CliError, Result, anonymous_session, config, prompt_secret, read_secret_stdin};

const MAXIMUM_BODY_BYTES: u64 = 1024 * 1024;

enum Prepared {
    Folder(DelegatedManifest<DelegatedCreateFolder>),
    List(DelegatedManifest<DelegatedListEntries>),
    Read(DelegatedManifest<DelegatedReadFile>),
    Trash(DelegatedManifest<DelegatedTrashEntry>),
    Reserve(DelegatedManifest<DelegatedReserveUpload>),
    Commit(DelegatedManifest<DelegatedCommitUpload>),
    Status(DelegatedManifest<DelegatedUploadQuery>),
    Cancel(DelegatedManifest<DelegatedCancelUpload>),
}

impl Prepared {
    fn read(operation: DelegatedOperation, path: &Path) -> Result<Self> {
        let file = open_regular_file(path, false)?;
        let mut bytes = Vec::new();
        file.take(MAXIMUM_BODY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| file_error(path, error))?;
        if bytes.len() as u64 > MAXIMUM_BODY_BYTES {
            return Err(CliError::usage(
                "delegated JSON bodies are limited to 1 MiB",
            ));
        }
        Ok(match operation {
            DelegatedOperation::FolderCreate => {
                Self::Folder(parse::<DelegatedCreateFolder>(&bytes)?.prepare()?)
            }
            DelegatedOperation::EntriesList => {
                Self::List(parse::<DelegatedListEntries>(&bytes)?.prepare()?)
            }
            DelegatedOperation::FileRead => {
                Self::Read(parse::<DelegatedReadFile>(&bytes)?.prepare()?)
            }
            DelegatedOperation::EntryTrash => {
                Self::Trash(parse::<DelegatedTrashEntry>(&bytes)?.prepare()?)
            }
            DelegatedOperation::UploadReserve => {
                Self::Reserve(parse::<DelegatedReserveUpload>(&bytes)?.prepare()?)
            }
            DelegatedOperation::UploadCommit => {
                Self::Commit(parse::<DelegatedCommitUpload>(&bytes)?.prepare()?)
            }
            DelegatedOperation::UploadStatus => {
                Self::Status(parse::<DelegatedUploadQuery>(&bytes)?.prepare()?)
            }
            DelegatedOperation::UploadCancel => {
                Self::Cancel(parse::<DelegatedCancelUpload>(&bytes)?.prepare()?)
            }
        })
    }

    fn describe(&self, output: Output) -> Result<()> {
        match self {
            Self::Folder(manifest) => describe(manifest, output),
            Self::List(manifest) => describe(manifest, output),
            Self::Read(manifest) => describe(manifest, output),
            Self::Trash(manifest) => describe(manifest, output),
            Self::Reserve(manifest) => describe(manifest, output),
            Self::Commit(manifest) => describe(manifest, output),
            Self::Status(manifest) => describe(manifest, output),
            Self::Cancel(manifest) => describe(manifest, output),
        }
    }

    async fn execute(
        &self,
        client: &Client,
        app: &ApplicationId,
        proof: OboProof,
        destination: Option<AtomicDestination>,
        capability_destination: Option<CapabilityDestination>,
        output: Output,
    ) -> Result<()> {
        match self {
            Self::Folder(manifest) => {
                let entry = client
                    .create_folder_on_behalf_of(app, proof, manifest)
                    .await?;
                if output.is_json() {
                    output.json(&entry);
                } else {
                    output.note(&format!("created {}", entry.path));
                }
            }
            Self::List(manifest) => {
                let page = client
                    .list_entries_on_behalf_of(app, proof, manifest)
                    .await?;
                output.entry_page(&page, true);
            }
            Self::Read(manifest) => {
                let destination = destination
                    .ok_or_else(|| CliError::usage("file-read requires --output <FILE>"))?;
                let stream = client.read_file_on_behalf_of(app, proof, manifest).await?;
                let path = destination.path.clone();
                let written = destination.write(stream).await?;
                if output.is_json() {
                    output.json(&serde_json::json!({ "path": path, "bytes": written }));
                } else {
                    output.note(&format!("saved {written} bytes to {}", path.display()));
                }
            }
            Self::Trash(manifest) => {
                client
                    .trash_entry_on_behalf_of(app, proof, manifest)
                    .await?;
                if output.is_json() {
                    output.json(&serde_json::json!({ "trashed": true }));
                } else {
                    output.note("moved to the recoverable bin");
                }
            }
            Self::Reserve(manifest) => {
                let destination = capability_destination.ok_or_else(|| {
                    CliError::usage("upload-reserve requires --capability-file <NEW_FILE>")
                })?;
                let reservation = client
                    .reserve_delegated_upload(app, proof, manifest)
                    .await?;
                if let Some(capability) = reservation.capability {
                    destination.save(&capability)?;
                } else {
                    eprintln!(
                        "briefcase: no capability was issued; the new private capability file remains empty"
                    );
                }
                upload_status(&reservation.status, output);
            }
            Self::Commit(manifest) => {
                let status = client.commit_delegated_upload(app, proof, manifest).await?;
                upload_status(&status, output);
            }
            Self::Status(manifest) => {
                let status = client.delegated_upload_status(app, proof, manifest).await?;
                upload_status(&status, output);
            }
            Self::Cancel(manifest) => {
                let status = client.cancel_delegated_upload(app, proof, manifest).await?;
                upload_status(&status, output);
            }
        }
        Ok(())
    }
}

/// Describes locally or executes one SDK operation without a member session.
pub(super) async fn run(global: &GlobalArgs, args: &AppRequestArgs, output: Output) -> Result<()> {
    let prepared = Prepared::read(args.operation, &args.body)?;
    // This branch precedes all profile, credential, client, and updater access.
    if args.describe {
        return prepared.describe(output);
    }
    let destination = match (args.operation, args.output.as_deref()) {
        (DelegatedOperation::FileRead, Some(path)) => {
            Some(AtomicDestination::new(path, args.force)?)
        }
        (DelegatedOperation::FileRead, None) => {
            return Err(CliError::usage("file-read requires --output <FILE>"));
        }
        (_, Some(_)) => {
            return Err(CliError::usage("--output is only supported for file-read"));
        }
        (_, None) => None,
    };
    let capability_destination = match (args.operation, args.capability_file.as_deref()) {
        (DelegatedOperation::UploadReserve, Some(path)) => Some(CapabilityDestination::new(path)?),
        (DelegatedOperation::UploadReserve, None) => {
            return Err(CliError::usage(
                "upload-reserve requires --capability-file <NEW_FILE>",
            ));
        }
        (_, Some(_)) => {
            return Err(CliError::usage(
                "--capability-file is only supported for upload-reserve",
            ));
        }
        (_, None) => None,
    };
    let app = args
        .app_id
        .as_ref()
        .ok_or_else(|| CliError::usage("--app-id is required when sending a request"))?;
    let session = anonymous_session(global)?;
    let client = if global.no_verify {
        Client::new_unchecked(config(&session)?)?
    } else {
        Client::connect(config(&session)?).await?
    };
    let proof = if args.proof_stdin {
        read_secret_stdin()?
    } else if let Some(proof) = &args.proof {
        proof.clone()
    } else {
        prompt_secret("Fresh IAM OBO access proof: ")?
    };
    let result = prepared
        .execute(
            &client,
            app,
            OboProof::new(proof)?,
            destination,
            capability_destination,
            output,
        )
        .await;
    if result.is_err() {
        eprintln!(
            "briefcase: no automatic retry was made. A retry needs a fresh proof; keep the body and any operation_id unchanged."
        );
    }
    result
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body)
        .map_err(|_| CliError::usage("--body does not match the selected operation's JSON schema"))
}

fn describe<T: ManifestOperation>(manifest: &DelegatedManifest<T>, output: Output) -> Result<()> {
    let body = std::str::from_utf8(manifest.body_bytes())
        .map_err(|_| CliError::usage("the prepared JSON body is not UTF-8"))?;
    output.json(&serde_json::json!({
        "method": manifest.method(),
        "path": manifest.path(),
        "endpoint_id": manifest.endpoint_id(),
        "body_sha256": manifest.body_sha256(),
        "metadata": {},
        "body": body,
    }));
    Ok(())
}

/// Preparation is deliberately local, including when no profile exists.
pub(super) async fn prepare_upload(
    operation_id: Uuid,
    parent_path: &str,
    file: &Path,
    output: Output,
) -> Result<()> {
    let request = DelegatedReserveUpload::file(operation_id, parent_path, file).await?;
    output.json(&request);
    Ok(())
}

/// A capability authorizes private byte transfer, never publication or reads.
pub(super) async fn transfer(
    global: &GlobalArgs,
    upload_id: Uuid,
    file: &Path,
    capability_file: Option<&Path>,
    capability_stdin: bool,
    output: Output,
) -> Result<()> {
    if upload_id.is_nil() {
        return Err(CliError::usage("upload_id must be a non-nil UUID"));
    }
    if !std::fs::metadata(file)
        .map_err(|error| file_error(file, error))?
        .is_file()
    {
        return Err(CliError::usage("the upload source must be a regular file"));
    }
    let capability = if let Some(path) = capability_file {
        read_capability(path)?
    } else if capability_stdin {
        read_secret_stdin()?
    } else {
        prompt_secret("Private upload capability: ")?
    };
    let capability = UploadCapability::new(capability)?;
    let session = anonymous_session(global)?;
    let client = if global.no_verify {
        Client::new_unchecked(config(&session)?)?
    } else {
        Client::connect(config(&session)?).await?
    };
    let result = client
        .transfer_delegated_upload(upload_id, capability, &UploadSource::File(file.to_owned()))
        .await;
    match result {
        Ok(status) => {
            upload_status(&status, output);
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "briefcase: transfer was not retried. Use upload-status with a fresh proof and the original operation_id to reconcile before retrying."
            );
            Err(error.into())
        }
    }
}

fn upload_status(status: &DelegatedUploadStatus, output: Output) {
    if output.is_json() {
        output.json(status);
    } else {
        output.note(&format!(
            "upload {}: {:?} (operation {})",
            status.upload_id, status.state, status.operation_id
        ));
        if let Some(entry_id) = status.published_entry_id {
            output.note(&format!("published entry {entry_id}"));
        }
    }
}

struct CapabilityDestination {
    path: PathBuf,
    file: File,
}

impl CapabilityDestination {
    fn new(path: &Path) -> Result<Self> {
        let file = create_private_file(path)?;
        eprintln!(
            "briefcase: reserved private capability file {}; it is retained if the request fails or no capability is issued",
            path.display()
        );
        Ok(Self {
            path: path.to_owned(),
            file,
        })
    }

    fn save(mut self, capability: &UploadCapability) -> Result<()> {
        self.file
            .write_all(capability.expose_secret().as_bytes())
            .and_then(|()| self.file.write_all(b"\n"))
            .and_then(|()| self.file.flush())
            .and_then(|()| self.file.sync_all())
            .map_err(|error| file_error(&self.path, error))?;
        eprintln!(
            "briefcase: saved private upload capability to {}",
            self.path.display()
        );
        Ok(())
    }
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| file_error(path, error))
}

#[cfg(not(unix))]
fn create_private_file(_path: &Path) -> Result<File> {
    Err(CliError::usage(
        "private capability files require a platform with owner-only Unix permissions",
    ))
}

#[cfg(unix)]
fn read_capability(path: &Path) -> Result<String> {
    let file = open_regular_file(path, true)?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| file_error(path, error))?;
    if bytes.len() > 64 * 1024 {
        return Err(CliError::usage("the capability file exceeds 64 KiB"));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| CliError::usage("the capability file is not UTF-8"))?;
    Ok(value.trim().to_owned())
}

#[cfg(not(unix))]
fn read_capability(_path: &Path) -> Result<String> {
    Err(CliError::usage(
        "private capability files require owner-only Unix permissions; use the hidden prompt or --capability-stdin",
    ))
}

/// Reject special files before opening, then bind validation to the opened FD.
/// Unix flags also reject a last-component symlink or FIFO substitution in the
/// gap between metadata and open, before any body or secret bytes are read.
fn open_regular_file(path: &Path, private: bool) -> Result<File> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| file_error(path, error))?;
    if !metadata.is_file() {
        return Err(CliError::usage("input must be a regular, non-symlink file"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        check_private_permissions(&metadata, private)?;
    }
    let file = options
        .open(path)
        .map_err(|error| file_error(path, error))?;
    let opened = file.metadata().map_err(|error| file_error(path, error))?;
    if !opened.is_file() {
        return Err(CliError::usage(
            "input changed while opening; nothing was read",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(CliError::usage(
                "input changed while opening; nothing was read",
            ));
        }
        check_private_permissions(&opened, private)?;
    }
    #[cfg(not(unix))]
    if private {
        return Err(CliError::usage(
            "owner-only input permissions cannot be verified",
        ));
    }
    Ok(file)
}

#[cfg(unix)]
fn check_private_permissions(metadata: &std::fs::Metadata, private: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if private && metadata.permissions().mode() & 0o077 != 0 {
        return Err(CliError::usage(
            "the capability file requires owner-only permissions (chmod 600)",
        ));
    }
    Ok(())
}

struct AtomicDestination {
    path: PathBuf,
    temporary: NamedTempFile,
    force: bool,
}

impl AtomicDestination {
    fn new(path: &Path, force: bool) -> Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(CliError::usage(
                    "--output must name a file, not a directory",
                ));
            }
            Ok(_) if !force => {
                return Err(CliError::usage(
                    "the output path already exists; use --force to replace it",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(file_error(path, error)),
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = tempfile::Builder::new()
            .prefix(".briefcase-download-")
            .tempfile_in(parent)
            .map_err(|error| file_error(path, error))?;
        Ok(Self {
            path: path.to_owned(),
            temporary,
            force,
        })
    }

    async fn write(self, mut stream: ContentStream) -> Result<u64> {
        let expected = stream.content_length();
        let file = self
            .temporary
            .reopen()
            .map_err(|error| file_error(&self.path, error))?;
        let mut file = tokio::fs::File::from_std(file);
        let mut written = 0_u64;
        while let Some(chunk) = stream.chunk().await? {
            file.write_all(&chunk)
                .await
                .map_err(|error| file_error(&self.path, error))?;
            written = written
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| CliError::usage("download size overflow"))?;
        }
        if expected.is_some_and(|expected| expected != written) {
            return Err(CliError::usage(
                "download length did not match the response; destination was not published",
            ));
        }
        file.flush()
            .await
            .map_err(|error| file_error(&self.path, error))?;
        file.sync_all()
            .await
            .map_err(|error| file_error(&self.path, error))?;
        drop(file);
        let persisted = if self.force {
            self.temporary.persist(&self.path)
        } else {
            self.temporary.persist_noclobber(&self.path)
        };
        persisted.map_err(|error| file_error(&self.path, error.error))?;
        Ok(written)
    }
}

fn file_error(path: &Path, source: std::io::Error) -> CliError {
    CliError::Io {
        path: path.display().to_string(),
        source,
    }
}
