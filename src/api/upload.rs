//! Bounded request-body staging for streaming uploads.

use std::{io, path::Path, path::PathBuf};

use axum::{
    body::Body,
    extract::multipart::{Field, MultipartError},
};
use futures::StreamExt as _;
use sha2::{Digest as _, Sha256};
use tempfile::TempPath;
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncWriteExt as _, BufWriter},
};

/// A private temporary upload and its verified stream metadata.
pub struct StagedUpload {
    path: TempPath,
    size: u64,
    sha256: [u8; 32],
}

impl StagedUpload {
    /// Borrows the temporary filesystem path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the exact received byte count.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the SHA-256 digest calculated while receiving the stream.
    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
}

/// Classified upload-staging failure.
#[derive(Debug, Error)]
pub enum StageUploadError {
    /// Received content exceeded the route limit.
    #[error("upload exceeds the route byte limit")]
    TooLarge,
    /// Multipart framing was malformed or interrupted.
    #[error("multipart upload stream is invalid")]
    Multipart(#[source] MultipartError),
    /// HTTP request-body streaming failed.
    #[error("request body stream failed")]
    Body(#[source] axum::Error),
    /// Private temporary storage failed.
    #[error("temporary upload storage failed")]
    Io(#[source] io::Error),
}

/// Streams an octet-stream request body into bounded temporary storage.
///
/// # Errors
///
/// Returns a classified staging error when the body exceeds the limit or its
/// stream or private temporary storage fails.
pub async fn stage_body(
    body: Body,
    temporary_directory: PathBuf,
    maximum_bytes: u64,
) -> Result<StagedUpload, StageUploadError> {
    let mut staging = StagingFile::create(temporary_directory, maximum_bytes).await?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        staging
            .write(&chunk.map_err(StageUploadError::Body)?)
            .await?;
    }
    staging.finish().await
}

/// Streams one multipart file field into bounded temporary storage.
///
/// # Errors
///
/// Returns a classified staging error when the field exceeds the limit or its
/// stream or private temporary storage fails.
pub async fn stage_multipart_field(
    mut field: Field<'_>,
    temporary_directory: PathBuf,
    maximum_bytes: u64,
) -> Result<StagedUpload, StageUploadError> {
    let mut staging = StagingFile::create(temporary_directory, maximum_bytes).await?;
    while let Some(chunk) = field.chunk().await.map_err(StageUploadError::Multipart)? {
        staging.write(&chunk).await?;
    }
    staging.finish().await
}

struct StagingFile {
    writer: BufWriter<File>,
    path: TempPath,
    size: u64,
    maximum_bytes: u64,
    digest: Sha256,
}

impl StagingFile {
    async fn create(
        temporary_directory: PathBuf,
        maximum_bytes: u64,
    ) -> Result<Self, StageUploadError> {
        tokio::fs::create_dir_all(&temporary_directory)
            .await
            .map_err(StageUploadError::Io)?;
        let temporary = tempfile::Builder::new()
            .prefix("briefcase-upload-")
            .tempfile_in(temporary_directory)
            .map_err(StageUploadError::Io)?;
        let (file, path) = temporary.into_parts();
        Ok(Self {
            writer: BufWriter::new(File::from_std(file)),
            path,
            size: 0,
            maximum_bytes,
            digest: Sha256::new(),
        })
    }

    async fn write(&mut self, chunk: &[u8]) -> Result<(), StageUploadError> {
        let chunk_size = u64::try_from(chunk.len()).map_err(|error| {
            StageUploadError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        let new_size = self
            .size
            .checked_add(chunk_size)
            .ok_or(StageUploadError::TooLarge)?;
        if new_size > self.maximum_bytes {
            return Err(StageUploadError::TooLarge);
        }
        self.writer
            .write_all(chunk)
            .await
            .map_err(StageUploadError::Io)?;
        self.digest.update(chunk);
        self.size = new_size;
        Ok(())
    }

    async fn finish(mut self) -> Result<StagedUpload, StageUploadError> {
        self.writer.flush().await.map_err(StageUploadError::Io)?;
        self.writer
            .get_ref()
            .sync_all()
            .await
            .map_err(StageUploadError::Io)?;
        let digest = self.digest.finalize();
        let mut sha256 = [0_u8; 32];
        sha256.copy_from_slice(&digest);
        Ok(StagedUpload {
            path: self.path,
            size: self.size,
            sha256,
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::{StageUploadError, stage_body};

    #[tokio::test]
    async fn stages_and_hashes_without_aggregating() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let staged = stage_body(Body::from("briefcase"), directory.path().to_owned(), 32).await?;

        assert_eq!(staged.size(), 9);
        assert_eq!(
            hex::encode(staged.sha256()),
            "645274c37697db3f009b17b5d2ff88437f6b63089618d39d424c6998088151da"
        );
        assert!(staged.path().exists());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_the_first_chunk_over_the_limit() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let result = stage_body(Body::from("oversized"), directory.path().to_owned(), 4).await;

        assert!(matches!(result, Err(StageUploadError::TooLarge)));
        Ok(())
    }
}
