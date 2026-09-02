//! The central notification inbox.
//!
//! A notification is an immutable record of something that changed what a
//! Carbon or Silicon can reach. It carries a snapshot of the entry it refers
//! to, because the recipient must still be able to read their own history
//! after losing access to that entry — or after it is permanently purged.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{
    actor::ActorRef,
    entry::{EntryKind, EntryPath},
    ids::{AccessRequestId, EntryId, NotificationId},
    permission::GrantedAccess,
};

/// How many notifications one inbox fetch returns.
pub const NOTIFICATION_PAGE_SIZE: u16 = 20;

/// What happened.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// The recipient received access to an entry.
    AccessGranted,
    /// The recipient's explicit access to an entry was revoked.
    AccessRevoked,
    /// Someone asked the recipient to decide an access request.
    AccessRequested,
    /// The recipient's own access request was decided.
    AccessRequestDecided,
}

/// The outcome recorded on a decided access request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDecision {
    /// The request was approved and a grant now exists.
    Approved,
    /// The request was denied.
    Denied,
}

/// The entry a notification refers to, as it was at that moment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntrySnapshot {
    /// Entry identifier.
    pub entry_id: EntryId,
    /// Display name at the time of the event.
    pub name: String,
    /// Organization-relative path at the time of the event.
    pub path: EntryPath,
    /// File or folder discriminator.
    pub kind: EntryKind,
}

/// One inbox entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    /// Notification identifier.
    pub id: NotificationId,
    /// What happened.
    pub kind: NotificationKind,
    /// Actor whose action produced the notification.
    pub actor: Option<ActorRef>,
    /// Entry the notification refers to.
    pub subject: Option<EntrySnapshot>,
    /// Rights involved in a grant, revocation, request, or approval.
    pub access: Option<GrantedAccess>,
    /// Access request that produced the notification.
    pub access_request_id: Option<AccessRequestId>,
    /// Outcome of a decided access request.
    pub decision: Option<NotificationDecision>,
    /// Creation time.
    pub created_at: OffsetDateTime,
    /// Time the recipient's inbox was marked read, when it has been.
    pub read_at: Option<OffsetDateTime>,
}

impl Notification {
    /// Returns whether the recipient has already read this notification.
    #[must_use]
    pub const fn is_read(&self) -> bool {
        self.read_at.is_some()
    }
}

/// One inbox fetch: the newest notifications plus the badge count.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationInbox {
    /// Newest-first notifications, at most [`NOTIFICATION_PAGE_SIZE`].
    pub items: Vec<Notification>,
    /// Number of unread notifications, used for the badge.
    pub unread_count: u64,
}
