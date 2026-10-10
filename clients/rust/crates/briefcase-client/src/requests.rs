//! What a caller asks for, in the shapes the operations accept.
//!
//! Every account has its own drive. A path names an entry inside one drive:
//! the caller's own unless the request names another by its owner's `c:`/`si:`
//! id (or uuid) — a drive shared with the caller, or the drive of a Silicon it
//! looks after.

use std::path::PathBuf;

use serde::Serialize;
use uuid::Uuid;

use crate::{
    client::IdempotencyKey,
    models::{AccessRight, ActorRef},
};

/// How a folder is named: by identifier, or by its path inside a drive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Destination {
    /// The folder's stable identifier (its drive comes with it).
    Id(Uuid),
    /// A path inside a drive; an empty path is the top of the drive.
    Path {
        /// The drive's owner, as a `c:`/`si:` id or uuid; the caller's own when absent.
        drive: Option<String>,
        /// The path within that drive, such as `notes/2026`.
        path: String,
    },
}

impl Destination {
    /// A folder in the caller's own drive, by path.
    #[must_use]
    pub fn path(path: impl Into<String>) -> Self {
        Self::Path {
            drive: None,
            path: normalize(&path.into()),
        }
    }

    /// A folder in another account's drive, such as `in_drive("si:scout", "reports")`.
    #[must_use]
    pub fn in_drive(drive: impl Into<String>, path: impl Into<String>) -> Self {
        Self::Path {
            drive: Some(drive.into()),
            path: normalize(&path.into()),
        }
    }

    /// The top of the caller's own drive.
    #[must_use]
    pub const fn top() -> Self {
        Self::Path {
            drive: None,
            path: String::new(),
        }
    }

    /// The top of another account's drive.
    #[must_use]
    pub fn top_of(drive: impl Into<String>) -> Self {
        Self::Path {
            drive: Some(drive.into()),
            path: String::new(),
        }
    }

    /// The drive this names, when it names one other than the caller's own.
    #[must_use]
    pub fn drive(&self) -> Option<&str> {
        match self {
            Self::Id(_) => None,
            Self::Path { drive, .. } => drive.as_deref(),
        }
    }
}

impl From<Uuid> for Destination {
    fn from(value: Uuid) -> Self {
        Self::Id(value)
    }
}

fn normalize(path: &str) -> String {
    path.trim().trim_matches('/').to_owned()
}

/// Which entries to list, and how many.
#[derive(Clone, Debug, Default)]
pub struct ListEntries {
    /// Folder to browse; the top of the caller's own drive when omitted.
    pub parent: Option<Destination>,
    /// Filter expression, applied within the folder's drive.
    pub filter: Option<String>,
    /// Cursor from a previous page.
    pub cursor: Option<String>,
    /// Page size, 1 through 100.
    pub limit: Option<u16>,
}

impl ListEntries {
    /// Lists the contents of one folder.
    #[must_use]
    pub fn in_folder(parent: impl Into<Destination>) -> Self {
        Self {
            parent: Some(parent.into()),
            ..Self::default()
        }
    }

    /// Filters the caller's own drive.
    #[must_use]
    pub fn matching(filter: impl Into<String>) -> Self {
        Self {
            filter: Some(filter.into()),
            ..Self::default()
        }
    }

    /// Continues from a previous page.
    #[must_use]
    pub fn after(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
        self
    }

    /// Asks for at most this many entries.
    #[must_use]
    pub const fn limit(mut self, limit: u16) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// A share to create, either on its own or with a new folder.
#[derive(Clone, Debug, Serialize)]
pub struct NewGrant {
    /// The account receiving access, named by its `c:`/`si:` id (or uuid).
    pub principal: ActorRef,
    /// Rights to convey; read is always included. Delete is never shared.
    pub access: Vec<AccessRight>,
    /// Whether the share reaches descendants.
    pub inherit: bool,
    /// Makes this a read-only expiring share that ends after 1 to 43,200 minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_minutes: Option<u32>,
}

impl NewGrant {
    /// Shares the entry itself with an account.
    #[must_use]
    pub fn new(principal: ActorRef, access: impl IntoIterator<Item = AccessRight>) -> Self {
        Self {
            principal,
            access: access.into_iter().collect(),
            inherit: false,
            expires_in_minutes: None,
        }
    }

    /// Makes the share an expiring share: read-only, and gone after `minutes`
    /// (1 to 43,200). It never replaces a permanent share the account holds.
    #[must_use]
    pub const fn expires_after(mut self, minutes: u32) -> Self {
        self.expires_in_minutes = Some(minutes);
        self
    }

    /// Extends the share to everything inside the entry.
    #[must_use]
    pub const fn inheriting(mut self) -> Self {
        self.inherit = true;
        self
    }
}

/// A folder to create.
#[derive(Clone, Debug)]
pub struct NewFolder {
    /// Display name.
    pub name: String,
    /// Destination folder; the top of the caller's own drive when omitted.
    pub parent: Option<Destination>,
    /// Accounts invited as the folder is created.
    pub invitees: Vec<NewGrant>,
    /// Key that makes a retry return the same folder.
    pub idempotency_key: Option<IdempotencyKey>,
}

impl NewFolder {
    /// Creates a folder inside another folder.
    #[must_use]
    pub fn in_folder(name: impl Into<String>, parent: impl Into<Destination>) -> Self {
        Self {
            name: name.into(),
            parent: Some(parent.into()),
            invitees: Vec::new(),
            idempotency_key: None,
        }
    }

    /// Creates a folder at the top of the caller's own drive.
    #[must_use]
    pub fn at_top(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parent: None,
            invitees: Vec::new(),
            idempotency_key: None,
        }
    }

    /// Invites accounts as the folder is created.
    #[must_use]
    pub fn inviting(mut self, invitees: impl IntoIterator<Item = NewGrant>) -> Self {
        self.invitees = invitees.into_iter().collect();
        self
    }

    /// Uses a caller-owned idempotency key.
    #[must_use]
    pub fn with_idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = Some(key);
        self
    }
}

/// A rename, a move within the same drive, or both.
#[derive(Clone, Debug, Default, Serialize)]
pub struct EntryUpdate {
    /// New display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New parent folder, in the same drive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Uuid>,
}

impl EntryUpdate {
    /// Renames an entry, leaving it where it is.
    #[must_use]
    pub fn rename(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            parent_id: None,
        }
    }

    /// Moves an entry into another folder of its drive, keeping its name.
    #[must_use]
    pub const fn move_to(parent_id: Uuid) -> Self {
        Self {
            name: None,
            parent_id: Some(parent_id),
        }
    }

    /// Also renames the entry being moved.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Where a file's bytes come from.
#[derive(Clone, Debug)]
pub enum UploadSource {
    /// A local file, streamed rather than read into memory.
    File(PathBuf),
    /// Bytes already in hand.
    Bytes(Vec<u8>),
}

/// A file to upload.
///
/// There is one upload operation for every size: Briefcase decides internally
/// whether the bytes travel as one provider request or a durable multipart
/// transfer. Uploading a name an active file already carries publishes that
/// file's next version. The bytes count against the drive they land in.
#[derive(Clone, Debug)]
pub struct Upload {
    /// Folder to upload into; [`Destination::top`] for the top of the drive.
    pub destination: Destination,
    /// Name the file takes in that folder.
    pub file_name: String,
    /// Media type; `application/octet-stream` when omitted.
    pub content_type: Option<String>,
    /// Where the bytes come from.
    pub source: UploadSource,
    /// Key that makes a retry return the same file rather than a second one.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Makes a new file self-destruct this many minutes (1 to 43,200) after
    /// the upload finishes. Refused when the name is an existing file.
    pub self_destruct_minutes: Option<u32>,
}

impl Upload {
    /// Uploads a local file, keeping its own name.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Configuration`] when the path has no file name.
    pub fn file(
        destination: impl Into<Destination>,
        path: impl Into<PathBuf>,
    ) -> crate::Result<Self> {
        let path = path.into();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| crate::Error::Configuration("the upload path has no file name".into()))?
            .to_owned();
        Ok(Self {
            destination: destination.into(),
            file_name,
            content_type: None,
            source: UploadSource::File(path),
            idempotency_key: None,
            self_destruct_minutes: None,
        })
    }

    /// Uploads bytes already in hand under a chosen name.
    #[must_use]
    pub fn bytes(
        destination: impl Into<Destination>,
        file_name: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            destination: destination.into(),
            file_name: file_name.into(),
            content_type: None,
            source: UploadSource::Bytes(bytes.into()),
            idempotency_key: None,
            self_destruct_minutes: None,
        }
    }

    /// Declares the media type of the bytes.
    #[must_use]
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Stores the file under a different name than the local one.
    #[must_use]
    pub fn named(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = file_name.into();
        self
    }

    /// Uses a caller-owned idempotency key.
    #[must_use]
    pub fn with_idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    /// Makes the new file self-destruct `minutes` (1 to 43,200) after the
    /// upload finishes. It is then deleted for good and never enters a bin.
    #[must_use]
    pub const fn self_destructing(mut self, minutes: u32) -> Self {
        self.self_destruct_minutes = Some(minutes);
        self
    }
}

/// One range of a file, for a player that seeks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    /// First byte, inclusive.
    pub start: u64,
    /// Last byte, inclusive; omitted means to the end.
    pub end: Option<u64>,
}

impl ByteRange {
    /// A range from an offset to the end of the file.
    #[must_use]
    pub const fn from(start: u64) -> Self {
        Self { start, end: None }
    }

    /// An inclusive range.
    #[must_use]
    pub const fn inclusive(start: u64, end: u64) -> Self {
        Self {
            start,
            end: Some(end),
        }
    }

    pub(crate) fn header_value(self) -> String {
        self.end.map_or_else(
            || format!("bytes={}-", self.start),
            |end| format!("bytes={}-{end}", self.start),
        )
    }
}

/// Targets whose effective access the caller wants reported.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PermissionQuery {
    /// Targets addressed by identifier.
    pub entry_ids: Vec<Uuid>,
    /// Targets addressed by path, inside `drive`.
    pub paths: Vec<String>,
    /// The drive the paths are in (its owner's `c:`/`si:` id or uuid); the
    /// caller's own when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive: Option<String>,
}

impl PermissionQuery {
    /// Asks about entries by identifier.
    #[must_use]
    pub fn entries(entry_ids: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            entry_ids: entry_ids.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Asks about entries by path in the caller's own drive.
    #[must_use]
    pub fn paths(paths: impl IntoIterator<Item = String>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Adds paths to a query that already names identifiers.
    #[must_use]
    pub fn and_paths(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.paths.extend(paths);
        self
    }

    /// Resolves the paths in another account's drive.
    #[must_use]
    pub fn in_drive(mut self, drive: impl Into<String>) -> Self {
        self.drive = Some(drive.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ByteRange, Destination, ListEntries, NewFolder, NewGrant, PermissionQuery, Upload,
    };
    use crate::models::{AccessRight, ActorRef};

    #[test]
    fn a_range_reads_as_the_header_a_player_sends() {
        assert_eq!(ByteRange::inclusive(0, 1023).header_value(), "bytes=0-1023");
        assert_eq!(ByteRange::from(2048).header_value(), "bytes=2048-");
    }

    #[test]
    fn destinations_name_a_drive_only_when_it_is_not_the_callers_own() {
        assert_eq!(
            Destination::path("/notes/2026/"),
            Destination::Path {
                drive: None,
                path: "notes/2026".to_owned()
            }
        );
        assert_eq!(Destination::path("notes").drive(), None);
        assert_eq!(
            Destination::in_drive("si:scout", "reports").drive(),
            Some("si:scout")
        );
        assert_eq!(
            Destination::top_of("c:ada"),
            Destination::Path {
                drive: Some("c:ada".to_owned()),
                path: String::new()
            }
        );
    }

    #[test]
    fn an_upload_takes_the_local_file_name_unless_told_otherwise() {
        let upload = Upload::file(Destination::path("notes"), "/tmp/report.pdf").unwrap();
        assert_eq!(upload.file_name, "report.pdf");
        assert_eq!(upload.named("q3.pdf").file_name, "q3.pdf");
        assert!(Upload::file(Destination::top(), "/").is_err());
    }

    #[test]
    fn folder_builders_carry_their_place_and_invitees() {
        let top = NewFolder::at_top("handbook");
        assert!(top.parent.is_none());
        let invited = NewFolder::in_folder("drafts", Destination::in_drive("si:scout", "notes"))
            .inviting([NewGrant::new(ActorRef::carbon("c:ada"), [AccessRight::Read]).inheriting()]);
        assert_eq!(invited.invitees.len(), 1);
        assert!(invited.invitees[0].inherit);
    }

    #[test]
    fn listing_and_inspection_builders_compose() {
        let query = ListEntries::matching("is:md").limit(5).after("cursor");
        assert_eq!(query.filter.as_deref(), Some("is:md"));
        assert_eq!(query.limit, Some(5));
        assert_eq!(query.cursor.as_deref(), Some("cursor"));
        let inspection = PermissionQuery::paths(["notes".to_owned()]).in_drive("c:ada");
        let body = serde_json::to_value(&inspection).unwrap();
        assert_eq!(body["drive"], "c:ada");
        assert!(serde_json::to_value(PermissionQuery::default()).unwrap()["drive"].is_null());
    }
}
