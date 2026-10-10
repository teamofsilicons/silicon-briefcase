//! Browsing, folders, addresses, and the recoverable bin.

use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    client::{Client, IdempotencyKey, json_body},
    error::{Error, Result},
    models::{ActivityEvent, ActivityPage, Entry, EntryPage, Moved, SearchPage, SearchResult},
    requests::{Destination, EntryUpdate, ListEntries, NewFolder, NewGrant},
};

#[derive(Serialize)]
struct WireFolder<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drive: Option<&'a str>,
    invitees: &'a [NewGrant],
}

impl Client {
    /// Lists a folder's contents, or filters a drive.
    ///
    /// Without a parent this lists the top of the caller's own drive; name
    /// another drive with [`Destination::top_of`] or [`Destination::in_drive`].
    /// Entries the caller may not see are already gone from the answer, and a
    /// page is refilled rather than answered short, so a full page means what
    /// it says. Follow `next_cursor` until it is absent to walk everything.
    ///
    /// # Errors
    ///
    /// Returns an error when the folder is not visible, the filter cannot be
    /// parsed, or the deployment cannot be reached.
    pub async fn list_entries(&self, query: &ListEntries) -> Result<EntryPage> {
        let mut url = self.api_url(&["entries"])?;
        {
            let mut pairs = url.query_pairs_mut();
            match &query.parent {
                Some(Destination::Id(id)) => {
                    pairs.append_pair("parent_id", &id.to_string());
                }
                Some(Destination::Path { drive, path }) => {
                    if let Some(drive) = drive {
                        pairs.append_pair("drive", drive);
                    }
                    if !path.is_empty() {
                        pairs.append_pair("path", path);
                    }
                }
                None => {}
            }
            if let Some(filter) = &query.filter {
                pairs.append_pair("filter", filter);
            }
            if let Some(cursor) = &query.cursor {
                pairs.append_pair("cursor", cursor);
            }
            if let Some(limit) = query.limit {
                pairs.append_pair("limit", &limit.to_string());
            }
        }
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Walks every page of a listing, gathering the entries.
    ///
    /// A convenience over [`Client::list_entries`] for callers that want the
    /// whole folder rather than one page. `max_entries` bounds the walk so a
    /// very large folder cannot surprise the caller.
    ///
    /// # Errors
    ///
    /// Returns the first error any page returns.
    pub async fn list_all_entries(
        &self,
        query: &ListEntries,
        max_entries: usize,
    ) -> Result<Vec<Entry>> {
        let mut gathered = Vec::new();
        let mut page_query = query.clone();
        loop {
            let page = self.list_entries(&page_query).await?;
            gathered.extend(page.items);
            let Some(cursor) = page.next_cursor.filter(|_| gathered.len() < max_entries) else {
                break;
            };
            page_query.cursor = Some(cursor);
        }
        gathered.truncate(max_entries);
        Ok(gathered)
    }

    /// Creates a folder, at the top of the caller's own drive when no parent
    /// is named.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination is not writable, or an invitee
    /// cannot be shared with (an unknown or inactive account, or a Silicon that
    /// has not allowed the caller: `silicon_not_open`).
    pub async fn create_folder(&self, folder: &NewFolder) -> Result<Entry> {
        let (parent_id, parent_path, drive) = match &folder.parent {
            Some(Destination::Id(id)) => (Some(*id), None, None),
            Some(Destination::Path { drive, path }) => (
                None,
                Some(path.as_str()).filter(|path| !path.is_empty()),
                drive.as_deref(),
            ),
            None => (None, None, None),
        };
        let body = json_body(&WireFolder {
            name: &folder.name,
            parent_id,
            parent_path,
            drive,
            invitees: &folder.invitees,
        })?;
        let key = folder
            .idempotency_key
            .clone()
            .unwrap_or_else(IdempotencyKey::random);
        let request = self
            .request(Method::POST, self.api_url(&["entries"])?)
            .header("content-type", "application/json")
            .header("idempotency-key", key.as_str())
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Reads one entry's metadata and the caller's access to it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for an entry that does not exist and for one
    /// the caller may not read; Briefcase does not distinguish them.
    pub async fn entry(&self, entry_id: Uuid) -> Result<Entry> {
        let url = self.api_url(&["entries", &entry_id.to_string()])?;
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Reads one entry by its address: the drive owner's `c:`/`si:` id or uuid
    /// and the path inside that drive (`GET /api/v1/paths/{owner}/{path}`). An
    /// empty path is the top of the drive.
    ///
    /// When the owner gave that id up, Briefcase answers `308` with the
    /// owner's current id; this follows it once, by the owner's uuid.
    ///
    /// # Errors
    ///
    /// Returns a not-found error when nothing the caller may read sits there.
    pub async fn entry_at(&self, owner: &str, path: &str) -> Result<Entry> {
        let response = self
            .resolve_address(owner, path, |client, url| {
                client
                    .request(Method::GET, url)
                    .timeout(client.request_timeout())
            })
            .await?;
        Self::decode_json(response).await
    }

    /// Reads one entry by an address from before personal drives,
    /// `/org/{org}/{path}`; Briefcase resolves it to the entry's current place
    /// (the entry's `permanent_url` is its address now).
    ///
    /// # Errors
    ///
    /// Returns a not-found error when the address names nothing the caller may read.
    pub async fn entry_at_legacy_address(&self, organization: &str, path: &str) -> Result<Entry> {
        let mut segments = vec!["org", organization];
        segments.extend(path.split('/').filter(|segment| !segment.is_empty()));
        let request = self
            .request(Method::GET, self.origin_url(&segments)?)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Sends a request to an address, following one `308` to the owner's
    /// current id by uuid, and returns the successful response.
    pub(crate) async fn resolve_address(
        &self,
        owner: &str,
        path: &str,
        build: impl Fn(&Self, url::Url) -> RequestBuilder,
    ) -> Result<Response> {
        let response = Self::send(build(self, self.path_url(owner, path)?)).await?;
        let response = if response.status() == StatusCode::PERMANENT_REDIRECT {
            let moved: Moved = Self::decode_json(response).await?;
            Self::send(build(self, self.path_url(&moved.owner.uuid, path)?)).await?
        } else {
            response
        };
        if response.status().is_success() {
            Ok(response)
        } else if response.status().is_redirection() {
            Err(Error::Protocol(
                "Briefcase moved this address twice in a row; resolve it again".into(),
            ))
        } else {
            Err(Self::refusal(response).await)
        }
    }

    /// Builds `/api/v1/paths/{owner}/{path}`, escaping each segment on its own.
    pub(crate) fn path_url(&self, owner: &str, path: &str) -> Result<url::Url> {
        let mut segments = vec!["paths", owner];
        segments.extend(path.split('/').filter(|segment| !segment.is_empty()));
        self.api_url(&segments)
    }

    /// Renames an entry, moves it within its drive, or both.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller lacks update access on the entry, or
    /// write access on the destination folder; a destination in another drive
    /// is refused with `cross_drive_move`.
    pub async fn update_entry(&self, entry_id: Uuid, update: &EntryUpdate) -> Result<Entry> {
        self.update_entry_with_key(entry_id, update, &IdempotencyKey::random())
            .await
    }

    /// Renames or moves an entry with a caller-owned retry identity.
    ///
    /// Persist `idempotency_key` before the first attempt and reuse it after an
    /// uncertain transport failure so the mutation cannot be applied twice.
    ///
    /// # Errors
    ///
    /// The same as [`Client::update_entry`].
    pub async fn update_entry_with_key(
        &self,
        entry_id: Uuid,
        update: &EntryUpdate,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Entry> {
        let url = self.api_url(&["entries", &entry_id.to_string()])?;
        let body = json_body(update)?;
        let request = self
            .request(Method::PATCH, url)
            .header("content-type", "application/json")
            .header("idempotency-key", idempotency_key.as_str())
            .body(body)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Moves an entry to its drive's bin, where it stays recoverable for 45 days.
    ///
    /// Deleting a folder takes everything inside it, and all of it comes back
    /// together on restore.
    ///
    /// # Errors
    ///
    /// Returns an error unless the caller created the entry, owns its drive, or
    /// is the custodian of the Silicon that did: delete is never shared.
    pub async fn delete_entry(&self, entry_id: Uuid) -> Result<()> {
        let url = self.api_url(&["entries", &entry_id.to_string()])?;
        let request = self
            .request(Method::DELETE, url)
            .timeout(self.request_timeout());
        self.receive_empty(request).await
    }

    /// Keeps a self-destructing file: stops its timer so it is never deleted.
    ///
    /// The file's creator, the drive's owner, and the custodian of either may
    /// keep a file.
    ///
    /// # Errors
    ///
    /// Returns a forbidden error for anyone else who can see the file, a
    /// not-found error when it is not visible, and `not_self_destructing` when
    /// its timer is not running.
    pub async fn make_permanent(&self, entry_id: Uuid) -> Result<()> {
        let url = self.api_url(&["entries", &entry_id.to_string(), "self-destruct"])?;
        let request = self
            .request(Method::DELETE, url)
            .timeout(self.request_timeout());
        self.receive_empty(request).await
    }

    /// Reads the retained "who did what, when" history of one entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is not visible to the caller.
    pub async fn activity(&self, entry_id: Uuid) -> Result<Vec<ActivityEvent>> {
        let url = self.api_url(&["entries", &entry_id.to_string(), "activity"])?;
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        let page: ActivityPage = self.receive_json(request).await?;
        Ok(page.items)
    }

    /// Searches filenames and extracted document text in the caller's drive,
    /// the drives of the Silicons it looks after, and what was shared with it.
    ///
    /// # Errors
    ///
    /// Returns an error when the query is empty or the limit is above twenty.
    pub async fn search(&self, query: &str, limit: Option<u8>) -> Result<Vec<SearchResult>> {
        let mut url = self.api_url(&["search"])?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", query);
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
        }
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        let page: SearchPage = self.receive_json(request).await?;
        Ok(page.items)
    }

    /// Lists a drive's bin (the caller's own when `drive` is absent), newest
    /// deletion first. The owner and a Silicon's custodian see everything in
    /// it; anyone else sees what they created there and what was shared with them.
    ///
    /// # Errors
    ///
    /// Returns an error when the limit is above a hundred or the cursor is not
    /// one Briefcase issued.
    pub async fn bin(
        &self,
        drive: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u16>,
    ) -> Result<EntryPage> {
        let mut url = self.api_url(&["bin"])?;
        {
            let mut pairs = url.query_pairs_mut();
            if let Some(drive) = drive {
                pairs.append_pair("drive", drive);
            }
            if let Some(cursor) = cursor {
                pairs.append_pair("cursor", cursor);
            }
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
        }
        let request = self
            .request(Method::GET, url)
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }

    /// Restores a deleted entry, with everything that was inside it.
    ///
    /// If the original folder is gone (or closed to the caller), the drive's
    /// owner gets it back at the top of the drive.
    ///
    /// # Errors
    ///
    /// Returns a not-found error once the 45-day window has passed.
    pub async fn restore_from_bin(&self, entry_id: Uuid) -> Result<Entry> {
        self.restore_from_bin_with_key(entry_id, &IdempotencyKey::random())
            .await
    }

    /// Restores a deleted entry with a stable operation identity.
    ///
    /// Keep the same key when recovering an uncertain response. A key belongs
    /// to one deletion cycle; use a new key after the entry is deleted again.
    ///
    /// # Errors
    ///
    /// Returns the API error if the entry cannot be restored, authorization
    /// fails, or the key belongs to a different deletion cycle.
    pub async fn restore_from_bin_with_key(
        &self,
        entry_id: Uuid,
        key: &IdempotencyKey,
    ) -> Result<Entry> {
        let url = self.api_url(&["bin", &entry_id.to_string(), "restore"])?;
        let request = self
            .request(Method::POST, url)
            .header("Idempotency-Key", key.as_str())
            .timeout(self.request_timeout());
        self.receive_json(request).await
    }
}
