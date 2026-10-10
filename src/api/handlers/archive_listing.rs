//! `GET /api/v1/entries/{entry_id}/archive`: what a zip, tar, tar.gz,
//! tar.zst or 7z file holds, listed without extracting anything.
//!
//! The caller needs read access to the file, exactly as for its content.
//! Zip archives are listed from their central directory (ranged reads of the
//! end of the file); tar archives are read as a stream within byte and entry
//! limits; 7z archives up to 512 MiB are copied to a private temporary file
//! to read their index. The listing reports whether it was cut short.

use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{PathRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse as _, Response},
};
use futures::StreamExt as _;
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;
use tokio_util::io::{StreamReader, SyncIoBridge};
use uuid::Uuid;

use super::super::{auth::Action, drives, extract, mapping::metadata_error, state::AppState};
use crate::{
    application::{
        archive_listing::{
            self, ArchiveError, ArchiveFormat, ArchiveListing, MAX_LISTED_ENTRIES,
            MAX_SEVEN_ZIP_BYTES, MAX_ZIP_DIRECTORY_BYTES, ZIP_TAIL_BYTES, ZipLocation,
        },
        content::ContentIntent,
        context::ExecutionContext,
        ports::RangeRequest,
    },
    domain::{entry::EntryKind, ids::EntryId},
    error::AppError,
};

/// Listings that read whole archives (tar streams, 7z copies) run a few at a
/// time; the others wait for a slot.
static LISTING_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// Query of an archive listing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArchiveQuery {
    /// The most entries to return (1 to 10,000; default 10,000).
    pub(crate) limit: Option<usize>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    path: Result<Path<Uuid>, PathRejection>,
    query: Result<Query<ArchiveQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let id = extract::path(path)?;
    let query = extract::query(query)?;
    let limit = query.limit.unwrap_or(MAX_LISTED_ENTRIES);
    if !(1..=MAX_LISTED_ENTRIES).contains(&limit) {
        return Err(AppError::validation("invalid_limit"));
    }
    let context =
        extract::authenticate(&state, &headers, Action::ReadContent, &id.to_string()).await?;
    let entry_id = extract::entry_id(id)?;
    let context = drives::for_entry(&state, context, entry_id).await?;
    let entry = extract::scoped(&context, state.metadata.get_entry(&context, entry_id))
        .await
        .map_err(metadata_error)?;
    let format = (entry.entry.kind == EntryKind::File)
        .then(|| {
            ArchiveFormat::detect(entry.entry.name.as_str(), entry.entry.content_type.as_deref())
        })
        .flatten()
        .ok_or_else(|| {
            AppError::unprocessable(
                "not_an_archive",
                format!(
                    "Briefcase lists zip, tar, tar.gz, tar.zst and 7z files; {} is not one of them.",
                    entry.entry.name.as_str()
                ),
            )
        })?;
    let listing = extract::scoped(&context, async {
        if format == ArchiveFormat::Zip {
            return list_zip(&state, &context, entry_id, limit).await;
        }
        let _slot = LISTING_SLOTS
            .acquire()
            .await
            .map_err(|_| AppError::Internal {
                category: "archive_listing_slots",
            })?;
        if format == ArchiveFormat::SevenZip {
            list_seven_zip(&state, &context, entry_id, limit).await
        } else {
            list_tar(&state, &context, entry_id, format, limit).await
        }
    })
    .await?;
    let mut response = Json(listing).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

/// Reads one byte range of the file into memory.
async fn read_range(
    state: &AppState,
    context: &ExecutionContext,
    entry_id: EntryId,
    range: RangeRequest,
) -> Result<(Vec<u8>, u64), AppError> {
    let delivery = state
        .content
        .open_content(context, entry_id, ContentIntent::Render, Some(range))
        .await
        .map_err(empty_as_malformed)?;
    let total = delivery.total_size;
    let mut bytes = Vec::new();
    let mut body = delivery.body;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| storage_failed())?;
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, total))
}

async fn list_zip(
    state: &AppState,
    context: &ExecutionContext,
    entry_id: EntryId,
    limit: usize,
) -> Result<ArchiveListing, AppError> {
    let (tail, total) =
        read_range(state, context, entry_id, RangeRequest::Last(ZIP_TAIL_BYTES)).await?;
    let tail_start = total - tail.len() as u64;
    let directory = match archive_listing::locate_zip_directory(&tail, total).map_err(refused)? {
        ZipLocation::Directory(directory) => directory,
        ZipLocation::Zip64Record(offset) => {
            let record = if offset >= tail_start && offset + 56 <= total {
                let at = usize::try_from(offset - tail_start).map_err(|_| storage_failed())?;
                tail[at..at + 56].to_vec()
            } else {
                read_range(
                    state,
                    context,
                    entry_id,
                    RangeRequest::Between(offset, offset + 55),
                )
                .await?
                .0
            };
            archive_listing::zip64_directory(&record).map_err(refused)?
        }
    };
    let wanted = directory.size.min(MAX_ZIP_DIRECTORY_BYTES);
    let bytes = if wanted == 0 {
        Vec::new()
    } else if directory.offset >= tail_start {
        let at = usize::try_from(directory.offset - tail_start).map_err(|_| storage_failed())?;
        let end = usize::try_from(directory.offset - tail_start + wanted)
            .map_err(|_| storage_failed())?;
        tail.get(at..end).map(<[u8]>::to_vec).unwrap_or_default()
    } else {
        read_range(
            state,
            context,
            entry_id,
            RangeRequest::Between(directory.offset, directory.offset + wanted - 1),
        )
        .await?
        .0
    };
    Ok(archive_listing::zip_entries(
        &bytes,
        directory.entries,
        limit,
    ))
}

async fn list_tar(
    state: &AppState,
    context: &ExecutionContext,
    entry_id: EntryId,
    format: ArchiveFormat,
    limit: usize,
) -> Result<ArchiveListing, AppError> {
    let delivery = state
        .content
        .open_content(context, entry_id, ContentIntent::Render, None)
        .await?;
    let reader = SyncIoBridge::new(StreamReader::new(delivery.body));
    tokio::task::spawn_blocking(move || archive_listing::tar_entries(format, reader, limit))
        .await
        .map_err(|_| AppError::Internal {
            category: "archive_listing",
        })?
        .map_err(refused)
}

async fn list_seven_zip(
    state: &AppState,
    context: &ExecutionContext,
    entry_id: EntryId,
    limit: usize,
) -> Result<ArchiveListing, AppError> {
    let delivery = state
        .content
        .open_content(context, entry_id, ContentIntent::Render, None)
        .await?;
    if delivery.total_size > MAX_SEVEN_ZIP_BYTES {
        return Err(refused(ArchiveError::TooLarge {
            limit: MAX_SEVEN_ZIP_BYTES,
        }));
    }
    tokio::fs::create_dir_all(&state.temporary_directory)
        .await
        .map_err(|_| temporary_failed())?;
    let copy = tempfile::Builder::new()
        .prefix("briefcase-archive-")
        .tempfile_in(&state.temporary_directory)
        .map_err(|_| temporary_failed())?;
    let (file, path) = copy.into_parts();
    let mut writer = tokio::fs::File::from_std(file);
    let mut body = delivery.body;
    let mut written = 0_u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| storage_failed())?;
        written += chunk.len() as u64;
        if written > MAX_SEVEN_ZIP_BYTES {
            return Err(refused(ArchiveError::TooLarge {
                limit: MAX_SEVEN_ZIP_BYTES,
            }));
        }
        writer
            .write_all(&chunk)
            .await
            .map_err(|_| temporary_failed())?;
    }
    writer.flush().await.map_err(|_| temporary_failed())?;
    drop(writer);
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path).map_err(|_| ArchiveError::Malformed("copy"))?;
        let listing = archive_listing::seven_zip_entries(&mut file, limit);
        drop(path);
        listing
    })
    .await
    .map_err(|_| AppError::Internal {
        category: "archive_listing",
    })?
    .map_err(refused)
}

fn refused(error: ArchiveError) -> AppError {
    match error {
        ArchiveError::Malformed(reason) => AppError::unprocessable(
            "archive_unreadable",
            format!(
                "The file is not a readable archive of its kind ({reason}); download it to inspect it."
            ),
        ),
        ArchiveError::Encrypted => AppError::unprocessable(
            "archive_encrypted",
            "The archive's list of contents is encrypted; download it and open it with its password.",
        ),
        ArchiveError::TooLarge { limit } => AppError::unprocessable(
            "archive_too_large_to_list",
            format!(
                "Briefcase lists archives of this kind up to {} MiB; download it to see its contents.",
                limit / (1024 * 1024)
            ),
        ),
    }
}

fn empty_as_malformed(error: AppError) -> AppError {
    match error {
        AppError::RangeNotSatisfiable { .. } => refused(ArchiveError::Malformed("empty_file")),
        other => other,
    }
}

const fn storage_failed() -> AppError {
    AppError::DependencyUnavailable {
        dependency: "object_storage",
    }
}

const fn temporary_failed() -> AppError {
    AppError::Internal {
        category: "temporary_archive_copy",
    }
}
