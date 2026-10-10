//! Listing what an archive holds without extracting anything.
//!
//! Briefcase shows the contents of zip, tar, tar.gz, tar.zst and 7z files.
//! Zip archives are read from their central directory at the end of the
//! file (a few ranged reads, whatever the archive's size); tar archives are
//! read as a stream, header by header, within byte and entry limits; 7z
//! archives keep their index at the end and are listed from a bounded local
//! copy. Names come from untrusted files: they are decoded leniently and
//! never used as paths on this machine.

use std::io::Read;

use serde::Serialize;
use time::OffsetDateTime;

/// The most entries one listing returns.
pub const MAX_LISTED_ENTRIES: usize = 10_000;
/// The longest central directory read from a zip archive: room for far more
/// than [`MAX_LISTED_ENTRIES`] entries even with long names.
pub const MAX_ZIP_DIRECTORY_BYTES: u64 = 8 * 1024 * 1024;
/// How much of the end of a zip archive is read to find its directory.
pub const ZIP_TAIL_BYTES: u64 = 65_557 + 20 + 56;
/// The most archive bytes a tar listing reads from storage.
pub const MAX_TAR_READ_BYTES: u64 = 1024 * 1024 * 1024;
/// The largest 7z archive copied locally to read its index.
pub const MAX_SEVEN_ZIP_BYTES: u64 = 512 * 1024 * 1024;
/// The longest name kept for one entry (characters).
const MAX_NAME_CHARS: usize = 4_096;

/// An archive format Briefcase lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ArchiveFormat {
    /// `.zip`
    #[serde(rename = "zip")]
    Zip,
    /// `.tar`
    #[serde(rename = "tar")]
    Tar,
    /// `.tar.gz`, `.tgz`
    #[serde(rename = "tar.gz")]
    TarGz,
    /// `.tar.zst`, `.tzst`
    #[serde(rename = "tar.zst")]
    TarZst,
    /// `.7z`
    #[serde(rename = "7z")]
    SevenZip,
}

impl ArchiveFormat {
    /// The format of a file, from its name first and its media type second.
    #[must_use]
    pub fn detect(name: &str, content_type: Option<&str>) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let by_name = [
            (".tar.gz", Self::TarGz),
            (".tgz", Self::TarGz),
            (".tar.zst", Self::TarZst),
            (".tar.zstd", Self::TarZst),
            (".tzst", Self::TarZst),
            (".tar", Self::Tar),
            (".zip", Self::Zip),
            (".7z", Self::SevenZip),
        ]
        .into_iter()
        .find(|(suffix, _)| lower.ends_with(suffix))
        .map(|(_, format)| format);
        by_name.or(match content_type.map(str::to_ascii_lowercase).as_deref() {
            Some("application/zip" | "application/x-zip-compressed") => Some(Self::Zip),
            Some("application/x-tar") => Some(Self::Tar),
            Some("application/x-7z-compressed") => Some(Self::SevenZip),
            _ => None,
        })
    }
}

/// What one entry of an archive is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveItemKind {
    /// A file.
    File,
    /// A folder.
    Folder,
    /// A symbolic or hard link.
    Link,
    /// Anything else a tar can hold (devices, pipes).
    Other,
}

/// One entry of an archive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveItem {
    /// The entry's path inside the archive.
    pub path: String,
    /// File, folder, link or other.
    #[serde(rename = "type")]
    pub kind: ArchiveItemKind,
    /// Size once extracted, in bytes.
    pub size: u64,
    /// Size inside the archive, when the format records it per entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_size: Option<u64>,
    /// Last modification, when recorded.
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub modified_at: Option<OffsetDateTime>,
    /// Whether the entry's content is encrypted.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
}

/// An archive's contents, possibly cut short.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveListing {
    /// The format read.
    pub format: ArchiveFormat,
    /// The entries, in archive order.
    pub entries: Vec<ArchiveItem>,
    /// Whether more entries exist than are listed.
    pub truncated: bool,
    /// Why the listing stops early, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_because: Option<&'static str>,
}

/// An archive that cannot be listed, and why.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    /// The bytes are not the format the name says.
    Malformed(&'static str),
    /// The archive needs a password to list (its index is encrypted).
    Encrypted,
    /// The archive is larger than Briefcase lists in that format.
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
}

fn clean_name(raw: &str) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .take(MAX_NAME_CHARS)
        .collect()
}

// ---------------------------------------------------------------- zip ----

/// Where a zip archive's central directory is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZipDirectory {
    /// Offset of the directory from the start of the archive.
    pub offset: u64,
    /// Length of the directory.
    pub size: u64,
    /// Entries the archive declares.
    pub entries: u64,
}

/// A second read a zip64 archive needs: its zip64 end record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZipLocation {
    /// The directory is known.
    Directory(ZipDirectory),
    /// Read 56 bytes at this offset (the zip64 end record), then call
    /// [`zip64_directory`].
    Zip64Record(u64),
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|slice| u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    bytes.get(at..at + 8).map(|slice| {
        let mut array = [0_u8; 8];
        array.copy_from_slice(slice);
        u64::from_le_bytes(array)
    })
}

/// Finds the central directory from the last bytes of a zip archive
/// (`tail`, the final [`ZIP_TAIL_BYTES`] or the whole file).
///
/// # Errors
///
/// Returns [`ArchiveError::Malformed`] when no end-of-directory record is
/// there.
pub fn locate_zip_directory(tail: &[u8], total_size: u64) -> Result<ZipLocation, ArchiveError> {
    const END: u32 = 0x0605_4b50;
    const LOCATOR: u32 = 0x0706_4b50;
    let tail_start = total_size.saturating_sub(tail.len() as u64);
    let end_at = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&at| {
            u32_at(tail, at) == Some(END)
                && u16_at(tail, at + 20)
                    .is_some_and(|comment| at + 22 + usize::from(comment) <= tail.len())
        })
        .ok_or(ArchiveError::Malformed("zip_end_record_missing"))?;
    let entries = u16_at(tail, end_at + 10).ok_or(ArchiveError::Malformed("zip_end_record"))?;
    let size = u32_at(tail, end_at + 12).ok_or(ArchiveError::Malformed("zip_end_record"))?;
    let offset = u32_at(tail, end_at + 16).ok_or(ArchiveError::Malformed("zip_end_record"))?;
    if entries == u16::MAX || size == u32::MAX || offset == u32::MAX {
        let locator_at = end_at
            .checked_sub(20)
            .ok_or(ArchiveError::Malformed("zip64_locator_missing"))?;
        if u32_at(tail, locator_at) != Some(LOCATOR) {
            return Err(ArchiveError::Malformed("zip64_locator_missing"));
        }
        let record =
            u64_at(tail, locator_at + 8).ok_or(ArchiveError::Malformed("zip64_locator"))?;
        return Ok(ZipLocation::Zip64Record(record));
    }
    let directory = ZipDirectory {
        offset: u64::from(offset),
        size: u64::from(size),
        entries: u64::from(entries),
    };
    if directory.offset + directory.size > tail_start + end_at as u64 {
        return Err(ArchiveError::Malformed("zip_directory_out_of_range"));
    }
    Ok(ZipLocation::Directory(directory))
}

/// Reads the zip64 end record (56 bytes at the offset [`ZipLocation::Zip64Record`] named).
///
/// # Errors
///
/// Returns [`ArchiveError::Malformed`] for anything but a zip64 end record.
pub fn zip64_directory(record: &[u8]) -> Result<ZipDirectory, ArchiveError> {
    if u32_at(record, 0) != Some(0x0606_4b50) {
        return Err(ArchiveError::Malformed("zip64_end_record_missing"));
    }
    Ok(ZipDirectory {
        entries: u64_at(record, 32).ok_or(ArchiveError::Malformed("zip64_end_record"))?,
        size: u64_at(record, 40).ok_or(ArchiveError::Malformed("zip64_end_record"))?,
        offset: u64_at(record, 48).ok_or(ArchiveError::Malformed("zip64_end_record"))?,
    })
}

/// Lists the entries of a zip central directory (`directory`, its bytes
/// from the start; it may be cut short, which truncates the listing).
#[must_use]
pub fn zip_entries(directory: &[u8], declared: u64, limit: usize) -> ArchiveListing {
    const HEADER: u32 = 0x0201_4b50;
    let mut entries = Vec::new();
    let mut at = 0_usize;
    let mut complete = true;
    while u32_at(directory, at) == Some(HEADER) {
        if entries.len() == limit {
            complete = false;
            break;
        }
        let Some(fixed) = directory.get(at..at + 46) else {
            complete = false;
            break;
        };
        let made_by_host = fixed[5];
        let flags = u16::from_le_bytes([fixed[8], fixed[9]]);
        let dos_time = u16::from_le_bytes([fixed[12], fixed[13]]);
        let dos_date = u16::from_le_bytes([fixed[14], fixed[15]]);
        let mut compressed = u64::from(u32::from_le_bytes([
            fixed[20], fixed[21], fixed[22], fixed[23],
        ]));
        let mut size = u64::from(u32::from_le_bytes([
            fixed[24], fixed[25], fixed[26], fixed[27],
        ]));
        let name_length = usize::from(u16::from_le_bytes([fixed[28], fixed[29]]));
        let extra_length = usize::from(u16::from_le_bytes([fixed[30], fixed[31]]));
        let comment_length = usize::from(u16::from_le_bytes([fixed[32], fixed[33]]));
        let external = u32::from_le_bytes([fixed[38], fixed[39], fixed[40], fixed[41]]);
        let name_at = at + 46;
        let extra_at = name_at + name_length;
        let next = extra_at + extra_length + comment_length;
        let (Some(name), Some(extra)) = (
            directory.get(name_at..extra_at),
            directory.get(extra_at..extra_at + extra_length),
        ) else {
            complete = false;
            break;
        };
        let mut modified_at = dos_datetime(dos_date, dos_time);
        read_extra_fields(extra, &mut size, &mut compressed, &mut modified_at);
        let path = if flags & 0x0800 != 0 {
            String::from_utf8_lossy(name).into_owned()
        } else {
            // Without the UTF-8 flag names are CP437; ASCII reads the same.
            name.iter()
                .map(|&byte| {
                    if byte.is_ascii() {
                        char::from(byte)
                    } else {
                        '\u{fffd}'
                    }
                })
                .collect()
        };
        let unix_mode = (made_by_host == 3).then_some(external >> 16);
        let kind = match unix_mode.map(|mode| mode & 0o170_000) {
            Some(0o040_000) => ArchiveItemKind::Folder,
            Some(0o120_000) => ArchiveItemKind::Link,
            _ if path.ends_with('/') || external & 0x10 != 0 => ArchiveItemKind::Folder,
            _ => ArchiveItemKind::File,
        };
        entries.push(ArchiveItem {
            path: clean_name(&path),
            kind,
            size,
            compressed_size: Some(compressed),
            modified_at,
            encrypted: flags & 1 == 1,
        });
        at = next;
    }
    let truncated = !complete || (entries.len() as u64) < declared;
    ArchiveListing {
        format: ArchiveFormat::Zip,
        entries,
        truncated,
        truncated_because: truncated.then_some(if (limit as u64) < declared {
            "entry_limit"
        } else {
            "directory_limit"
        }),
    }
}

/// Applies the extra fields a listing uses: zip64 sizes (0x0001) and the
/// Unix modification time (0x5455).
fn read_extra_fields(
    extra: &[u8],
    size: &mut u64,
    compressed: &mut u64,
    modified_at: &mut Option<OffsetDateTime>,
) {
    let mut field = 0_usize;
    while let (Some(id), Some(length)) = (u16_at(extra, field), u16_at(extra, field + 2)) {
        let data = extra
            .get(field + 4..field + 4 + usize::from(length))
            .unwrap_or_default();
        if id == 0x0001 {
            let mut cursor = 0_usize;
            if *size == u64::from(u32::MAX)
                && let Some(value) = u64_at(data, cursor)
            {
                *size = value;
                cursor += 8;
            }
            if *compressed == u64::from(u32::MAX)
                && let Some(value) = u64_at(data, cursor)
            {
                *compressed = value;
            }
        } else if id == 0x5455
            && data.first().is_some_and(|flags| flags & 1 == 1)
            && let Some(seconds) = u32_at(data, 1)
        {
            *modified_at = OffsetDateTime::from_unix_timestamp(i64::from(seconds)).ok();
        }
        field += 4 + usize::from(length);
    }
}

fn dos_datetime(date: u16, time: u16) -> Option<OffsetDateTime> {
    let month = time::Month::try_from(u8::try_from((date >> 5) & 0x0f).ok()?).ok()?;
    let day = u8::try_from(date & 0x1f).ok()?;
    let year = 1980 + i32::from(date >> 9);
    let calendar = time::Date::from_calendar_date(year, month, day).ok()?;
    let clock = time::Time::from_hms(
        u8::try_from(time >> 11).ok()?,
        u8::try_from((time >> 5) & 0x3f).ok()?,
        u8::try_from((time & 0x1f) * 2).ok()?,
    )
    .ok()?;
    Some(calendar.with_time(clock).assume_utc())
}

// ---------------------------------------------------------------- tar ----

/// Reads at most `limit` bytes and then reports the end of the stream.
struct Capped<R> {
    inner: R,
    left: u64,
    hit: bool,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            self.hit = true;
            return Ok(0);
        }
        let allowed = usize::try_from(self.left)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.left -= read as u64;
        Ok(read)
    }
}

/// Lists a tar archive (plain, gzip or zstd) from a reader of its bytes,
/// reading at most [`MAX_TAR_READ_BYTES`] of them.
///
/// # Errors
///
/// Returns [`ArchiveError::Malformed`] when the stream is not a tar archive
/// at all.
pub fn tar_entries(
    format: ArchiveFormat,
    archive: impl Read,
    limit: usize,
) -> Result<ArchiveListing, ArchiveError> {
    tar_entries_within(format, archive, limit, MAX_TAR_READ_BYTES)
}

fn tar_entries_within(
    format: ArchiveFormat,
    archive: impl Read,
    limit: usize,
    read_limit: u64,
) -> Result<ArchiveListing, ArchiveError> {
    let mut capped = Capped {
        inner: archive,
        left: read_limit,
        hit: false,
    };
    let decoded: Box<dyn Read + '_> = match format {
        ArchiveFormat::TarGz => Box::new(flate2::read::MultiGzDecoder::new(&mut capped)),
        ArchiveFormat::TarZst => Box::new(
            zstd::stream::read::Decoder::new(&mut capped)
                .map_err(|_| ArchiveError::Malformed("zstd_stream"))?,
        ),
        _ => Box::new(&mut capped),
    };
    let mut reader = tar::Archive::new(decoded);
    let mut entries = Vec::new();
    let mut stopped: Option<&'static str> = None;
    let iterator = reader
        .entries()
        .map_err(|_| ArchiveError::Malformed("tar_stream"))?;
    for entry in iterator {
        if entries.len() == limit {
            stopped = Some("entry_limit");
            break;
        }
        let Ok(entry) = entry else {
            stopped = Some("unreadable_rest");
            break;
        };
        let header = entry.header();
        let kind = match header.entry_type() {
            tar::EntryType::Directory => ArchiveItemKind::Folder,
            tar::EntryType::Symlink | tar::EntryType::Link => ArchiveItemKind::Link,
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse => {
                ArchiveItemKind::File
            }
            _ => ArchiveItemKind::Other,
        };
        let path = entry.path_bytes().into_owned();
        entries.push(ArchiveItem {
            path: clean_name(&String::from_utf8_lossy(&path)),
            kind,
            size: header.size().unwrap_or_default(),
            compressed_size: None,
            modified_at: header.mtime().ok().and_then(|seconds| {
                OffsetDateTime::from_unix_timestamp(i64::try_from(seconds).ok()?).ok()
            }),
            encrypted: false,
        });
    }
    if entries.is_empty() && stopped == Some("unreadable_rest") {
        return Err(ArchiveError::Malformed("tar_stream"));
    }
    drop(reader);
    if capped.hit && stopped.is_none_or(|reason| reason == "unreadable_rest") {
        stopped = Some("read_limit");
    }
    Ok(ArchiveListing {
        format,
        entries,
        truncated: stopped.is_some(),
        truncated_because: stopped,
    })
}

// ----------------------------------------------------------------- 7z ----

/// Lists a 7z archive from a local copy of it.
///
/// # Errors
///
/// Returns [`ArchiveError::Encrypted`] when the index needs a password and
/// [`ArchiveError::Malformed`] when the file is not a 7z archive.
pub fn seven_zip_entries(
    archive: &mut (impl Read + std::io::Seek),
    limit: usize,
) -> Result<ArchiveListing, ArchiveError> {
    let index = sevenz_rust2::Archive::read(archive, &sevenz_rust2::Password::empty()).map_err(
        |error| match error {
            sevenz_rust2::Error::PasswordRequired | sevenz_rust2::Error::MaybeBadPassword(_) => {
                ArchiveError::Encrypted
            }
            _ => ArchiveError::Malformed("seven_zip_index"),
        },
    )?;
    let declared = index.files.len();
    let entries = index
        .files
        .iter()
        .filter(|entry| !entry.is_anti_item)
        .take(limit)
        .map(|entry| ArchiveItem {
            path: clean_name(entry.name()),
            kind: if entry.is_directory() {
                ArchiveItemKind::Folder
            } else {
                ArchiveItemKind::File
            },
            size: entry.size(),
            compressed_size: None,
            modified_at: entry.has_last_modified_date.then(|| {
                OffsetDateTime::from(std::time::SystemTime::from(entry.last_modified_date))
            }),
            encrypted: false,
        })
        .collect::<Vec<_>>();
    let truncated = entries.len() < declared && declared > limit;
    Ok(ArchiveListing {
        format: ArchiveFormat::SevenZip,
        entries,
        truncated,
        truncated_because: truncated.then_some("entry_limit"),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use super::{
        ArchiveError, ArchiveFormat, ArchiveItemKind, ZIP_TAIL_BYTES, ZipLocation,
        locate_zip_directory, seven_zip_entries, tar_entries, tar_entries_within, zip_entries,
        zip64_directory,
    };

    fn zip_archive(large: bool) -> anyhow::Result<Vec<u8>> {
        use zip::{CompressionMethod, write::SimpleFileOptions};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        writer.add_directory("docs/", options)?;
        writer.start_file("docs/readme.md", options.large_file(large))?;
        writer.write_all(b"# Read me")?;
        writer.start_file("naïve café.txt", options)?;
        writer.write_all(b"bonjour")?;
        writer.add_symlink("latest", "docs/readme.md", options)?;
        Ok(writer.finish()?.into_inner())
    }

    fn tail(bytes: &[u8]) -> &[u8] {
        let keep = usize::try_from(ZIP_TAIL_BYTES)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        &bytes[bytes.len() - keep..]
    }

    #[test]
    fn zip_archives_are_listed_from_their_central_directory() -> anyhow::Result<()> {
        for large in [false, true] {
            let bytes = zip_archive(large)?;
            let ZipLocation::Directory(directory) =
                locate_zip_directory(tail(&bytes), bytes.len() as u64)
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?
            else {
                anyhow::bail!("a small archive needs no zip64 record");
            };
            assert_eq!(directory.entries, 4);
            let start = usize::try_from(directory.offset)?;
            let end = start + usize::try_from(directory.size)?;
            let listing = zip_entries(&bytes[start..end], directory.entries, 100);
            assert!(!listing.truncated);
            let found: Vec<(&str, ArchiveItemKind, u64)> = listing
                .entries
                .iter()
                .map(|item| (item.path.as_str(), item.kind, item.size))
                .collect();
            assert_eq!(
                found,
                vec![
                    ("docs/", ArchiveItemKind::Folder, 0),
                    ("docs/readme.md", ArchiveItemKind::File, 9),
                    ("naïve café.txt", ArchiveItemKind::File, 7),
                    ("latest", ArchiveItemKind::Link, 14),
                ]
            );
            assert!(
                listing
                    .entries
                    .iter()
                    .all(|item| item.modified_at.is_some())
            );
            let cut = zip_entries(&bytes[start..end], directory.entries, 2);
            assert!(cut.truncated);
            assert_eq!(cut.entries.len(), 2);
            assert_eq!(cut.truncated_because, Some("entry_limit"));
            let short = zip_entries(&bytes[start..end - 10], directory.entries, 100);
            assert!(short.truncated, "a directory cut short says so");
        }
        Ok(())
    }

    #[test]
    fn zip64_archives_name_their_directory_in_the_zip64_record() -> anyhow::Result<()> {
        // A zip64 end record, its locator, and an end record whose fields
        // say "see zip64".
        let mut record = Vec::new();
        record.extend_from_slice(&0x0606_4b50_u32.to_le_bytes());
        record.extend_from_slice(&44_u64.to_le_bytes());
        record.extend_from_slice(&[45, 0, 45, 0]);
        record.extend_from_slice(&[0; 8]);
        record.extend_from_slice(&70_000_u64.to_le_bytes());
        record.extend_from_slice(&70_000_u64.to_le_bytes());
        record.extend_from_slice(&9_000_000_u64.to_le_bytes());
        record.extend_from_slice(&5_000_000_000_u64.to_le_bytes());
        let mut tail_bytes = record.clone();
        tail_bytes.extend_from_slice(&0x0706_4b50_u32.to_le_bytes());
        tail_bytes.extend_from_slice(&0_u32.to_le_bytes());
        tail_bytes.extend_from_slice(&5_009_000_000_u64.to_le_bytes());
        tail_bytes.extend_from_slice(&1_u32.to_le_bytes());
        tail_bytes.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
        tail_bytes.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        tail_bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        tail_bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        tail_bytes.extend_from_slice(&[0, 0]);
        let total = 5_009_000_000 + tail_bytes.len() as u64;
        let location = locate_zip_directory(&tail_bytes, total)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(location, ZipLocation::Zip64Record(5_009_000_000));
        let directory = zip64_directory(&record).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(directory.entries, 70_000);
        assert_eq!(directory.size, 9_000_000);
        assert_eq!(directory.offset, 5_000_000_000);
        Ok(())
    }

    #[test]
    fn what_is_not_a_zip_is_refused() {
        assert_eq!(
            locate_zip_directory(b"just some text, not an archive", 30),
            Err(ArchiveError::Malformed("zip_end_record_missing"))
        );
        assert!(zip64_directory(&[0; 56]).is_err());
    }

    fn tar_archive() -> anyhow::Result<Vec<u8>> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_mode(0o755);
        header.set_mtime(1_760_000_000);
        header.set_size(0);
        builder.append_data(&mut header, "photos/", std::io::empty())?;
        for (name, body) in [
            ("photos/beach.jpg", &b"jpeg bytes"[..]),
            (
                "photos/a folder with a very long name that does not fit the old one hundred byte tar field/sunset.jpg",
                &b"sunset"[..],
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_mtime(1_760_000_000);
            header.set_size(body.len() as u64);
            builder.append_data(&mut header, name, body)?;
        }
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder.append_link(&mut header, "photos/latest.jpg", "beach.jpg")?;
        Ok(builder.into_inner()?)
    }

    #[test]
    fn tar_archives_are_listed_header_by_header_in_every_compression() -> anyhow::Result<()> {
        let plain = tar_archive()?;
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&plain)?;
        let gzip = gzip.finish()?;
        let zstd = zstd::stream::encode_all(Cursor::new(&plain), 3)?;
        for (format, bytes) in [
            (ArchiveFormat::Tar, plain.clone()),
            (ArchiveFormat::TarGz, gzip),
            (ArchiveFormat::TarZst, zstd),
        ] {
            let listing = tar_entries(format, Cursor::new(bytes), 100)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?;
            assert!(!listing.truncated, "{format:?}");
            let found: Vec<(&str, ArchiveItemKind, u64)> = listing
                .entries
                .iter()
                .map(|item| (item.path.as_str(), item.kind, item.size))
                .collect();
            assert_eq!(found.len(), 4, "{format:?}: {found:?}");
            assert_eq!(found[0], ("photos/", ArchiveItemKind::Folder, 0));
            assert_eq!(found[1], ("photos/beach.jpg", ArchiveItemKind::File, 10));
            assert!(found[2].0.ends_with("/sunset.jpg") && found[2].0.len() > 100);
            assert_eq!(found[3].1, ArchiveItemKind::Link);
            assert_eq!(
                listing.entries[1]
                    .modified_at
                    .map(time::OffsetDateTime::unix_timestamp),
                Some(1_760_000_000)
            );
        }
        let cut = tar_entries(ArchiveFormat::Tar, Cursor::new(plain.clone()), 2)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(
            (cut.entries.len(), cut.truncated_because),
            (2, Some("entry_limit"))
        );
        let capped = tar_entries_within(ArchiveFormat::Tar, Cursor::new(plain), 100, 1_536)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert!(capped.truncated);
        assert_eq!(capped.truncated_because, Some("read_limit"));
        assert!(
            tar_entries(
                ArchiveFormat::TarGz,
                Cursor::new(b"not gzip at all".to_vec()),
                10
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn seven_zip_archives_are_listed_from_their_index() -> anyhow::Result<()> {
        let source = tempfile::tempdir()?;
        std::fs::create_dir(source.path().join("notes"))?;
        std::fs::write(source.path().join("notes/plan.md"), b"# Plan")?;
        std::fs::write(source.path().join("budget.csv"), b"a,b\n1,2\n")?;
        let target = tempfile::tempdir()?;
        let archive = target.path().join("bundle.7z");
        sevenz_rust2::compress_to_path(source.path(), &archive)?;
        let listing = seven_zip_entries(&mut std::fs::File::open(&archive)?, 100)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let mut found: Vec<(String, ArchiveItemKind, u64)> = listing
            .entries
            .iter()
            .map(|item| (item.path.clone(), item.kind, item.size))
            .collect();
        found.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            found,
            vec![
                ("budget.csv".to_owned(), ArchiveItemKind::File, 8),
                ("notes".to_owned(), ArchiveItemKind::Folder, 0),
                ("notes/plan.md".to_owned(), ArchiveItemKind::File, 6),
            ]
        );
        assert!(seven_zip_entries(&mut Cursor::new(b"not 7z".to_vec()), 10).is_err());
        Ok(())
    }

    #[test]
    fn formats_are_known_by_name_then_media_type() {
        for (name, content_type, expected) in [
            ("photos.zip", None, Some(ArchiveFormat::Zip)),
            ("Backup.TAR.GZ", None, Some(ArchiveFormat::TarGz)),
            ("logs.tgz", None, Some(ArchiveFormat::TarGz)),
            ("logs.tar.zst", None, Some(ArchiveFormat::TarZst)),
            ("src.tar", None, Some(ArchiveFormat::Tar)),
            ("bundle.7z", None, Some(ArchiveFormat::SevenZip)),
            (
                "download",
                Some("application/zip"),
                Some(ArchiveFormat::Zip),
            ),
            ("notes.md", Some("text/markdown"), None),
            ("report.docx", None, None),
        ] {
            assert_eq!(
                ArchiveFormat::detect(name, content_type),
                expected,
                "{name}"
            );
        }
    }
}
