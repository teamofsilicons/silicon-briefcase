//! Transactional writes and reads for the notification inbox.
//!
//! Every notification is written in the same transaction as the permission
//! change that caused it, so the inbox can never claim access that was rolled
//! back — or miss access that was committed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    application::context::ExecutionContext,
    domain::{
        actor::ActorRef,
        entry::{EntryKind, EntryPath},
        ids::{AccessRequestId, EntryId, NotificationId},
        notification::{
            EntrySnapshot, NOTIFICATION_PAGE_SIZE, Notification, NotificationDecision,
            NotificationInbox, NotificationKind,
        },
        permission::GrantedAccess,
    },
};

use super::common::{
    Result, actor_kind, actor_ref, decode_access, encode_access, entry_kind, internal, map_sql,
};

/// A notification about to be written alongside its cause.
#[derive(Clone, Copy, Debug)]
pub(in crate::infrastructure::postgres) struct NewNotification<'a> {
    /// Member whose inbox receives it.
    pub recipient: &'a ActorRef,
    /// What happened.
    pub kind: NotificationKind,
    /// Actor whose action produced it.
    pub actor: Option<&'a ActorRef>,
    /// Entry snapshot as it was at that moment.
    pub subject: Option<&'a EntrySnapshot>,
    /// Rights involved.
    pub access: Option<GrantedAccess>,
    /// Access request that produced it.
    pub access_request_id: Option<AccessRequestId>,
    /// Outcome of a decided access request.
    pub decision: Option<NotificationDecision>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct NotificationDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entry_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_mask: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_request_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decision: Option<NotificationDecision>,
}

#[derive(sqlx::FromRow)]
struct NotificationRow {
    notification_id: Uuid,
    kind: String,
    actor_type: Option<String>,
    actor_id: Option<String>,
    entry_id: Option<Uuid>,
    details: Value,
    read_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
}

/// Builds the durable snapshot a notification keeps about an entry.
pub(in crate::infrastructure::postgres) fn snapshot(
    entry: &crate::application::service::EntryView,
) -> EntrySnapshot {
    EntrySnapshot {
        entry_id: entry.id,
        name: entry.name.as_str().to_owned(),
        path: entry.path.clone(),
        kind: entry.kind,
    }
}

/// Writes one notification into a recipient's inbox.
pub(in crate::infrastructure::postgres) async fn insert(
    transaction: &mut Transaction<'_, Postgres>,
    notification: &NewNotification<'_>,
) -> Result<()> {
    let details = NotificationDetails {
        name: notification.subject.map(|subject| subject.name.clone()),
        path: notification
            .subject
            .map(|subject| subject.path.as_str().to_owned()),
        entry_type: notification
            .subject
            .map(|subject| encode_entry_kind(subject.kind).to_owned()),
        access_mask: notification.access.map(encode_access),
        access_request_id: notification.access_request_id.map(AccessRequestId::as_uuid),
        decision: notification.decision,
    };
    let details = serde_json::to_value(&details)
        .map_err(|_| internal("notification details cannot be encoded"))?;

    sqlx::query(
        "INSERT INTO briefcase.notifications ( \
                org_id, notification_id, recipient_type, recipient_id, kind, \
                actor_type, actor_id, entry_id, details \
         ) VALUES (briefcase.current_org_id(), $1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(NotificationId::new().as_uuid())
    .bind(actor_kind(notification.recipient.kind()))
    .bind(notification.recipient.id().as_str())
    .bind(encode_kind(notification.kind))
    .bind(notification.actor.map(|actor| actor_kind(actor.kind())))
    .bind(notification.actor.map(|actor| actor.id().as_str()))
    .bind(
        notification
            .subject
            .map(|subject| subject.entry_id.as_uuid()),
    )
    .bind(details)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    Ok(())
}

/// Returns the members who decide access requests for one entry.
///
/// The product contract routes a request to the entry owner and to every
/// organization owner and admin, because those are exactly the identities that
/// can approve it.
pub(in crate::infrastructure::postgres) async fn decision_recipients(
    transaction: &mut Transaction<'_, Postgres>,
    owner: &ActorRef,
    requester: &ActorRef,
) -> Result<Vec<ActorRef>> {
    #[derive(sqlx::FromRow)]
    struct RecipientRow {
        actor_type: String,
        actor_id: String,
    }

    let rows = sqlx::query_as::<_, RecipientRow>(
        "SELECT member.actor_type, member.actor_id \
           FROM briefcase.organization_members AS member \
          WHERE member.org_id = briefcase.current_org_id() \
            AND member.membership_status = 'active' \
            AND ( \
                member.org_role IN ('owner', 'admin') \
                OR (member.actor_type = $1 AND member.actor_id = $2) \
            ) \
            AND NOT (member.actor_type = $3 AND member.actor_id = $4) \
          ORDER BY member.actor_type, member.actor_id",
    )
    .bind(actor_kind(owner.kind()))
    .bind(owner.id().as_str())
    .bind(actor_kind(requester.kind()))
    .bind(requester.id().as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql)?;

    rows.iter()
        .map(|row| actor_ref(&row.actor_type, &row.actor_id))
        .collect()
}

/// Loads the caller's newest notifications and unread badge count.
pub(in crate::infrastructure::postgres) async fn load_inbox(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
) -> Result<NotificationInbox> {
    let actor = context.authorization().actor();
    let rows = sqlx::query_as::<_, NotificationRow>(
        "SELECT notification_id, kind, actor_type, actor_id, entry_id, details, \
                read_at, created_at \
           FROM briefcase.notifications \
          WHERE org_id = briefcase.current_org_id() \
            AND recipient_type = $1 AND recipient_id = $2 \
          ORDER BY created_at DESC, notification_id DESC \
          LIMIT $3",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .bind(i64::from(NOTIFICATION_PAGE_SIZE))
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql)?;

    let unread_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM briefcase.notifications \
          WHERE org_id = briefcase.current_org_id() \
            AND recipient_type = $1 AND recipient_id = $2 \
            AND read_at IS NULL",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_sql)?;

    let items = rows
        .into_iter()
        .map(notification)
        .collect::<Result<Vec<_>>>()?;
    Ok(NotificationInbox {
        items,
        unread_count: u64::try_from(unread_count)
            .map_err(|_| internal("negative notification count"))?,
    })
}

/// Marks every unread notification of the caller read.
pub(in crate::infrastructure::postgres) async fn mark_all_read(
    transaction: &mut Transaction<'_, Postgres>,
    context: &ExecutionContext,
) -> Result<()> {
    let actor = context.authorization().actor();
    sqlx::query(
        "UPDATE briefcase.notifications \
            SET read_at = clock_timestamp() \
          WHERE org_id = briefcase.current_org_id() \
            AND recipient_type = $1 AND recipient_id = $2 \
            AND read_at IS NULL",
    )
    .bind(actor_kind(actor.kind()))
    .bind(actor.id().as_str())
    .execute(&mut **transaction)
    .await
    .map_err(map_sql)?;
    Ok(())
}

fn notification(row: NotificationRow) -> Result<Notification> {
    let details: NotificationDetails = serde_json::from_value(row.details)
        .map_err(|_| internal("invalid persisted notification details"))?;
    let actor = match (row.actor_type.as_deref(), row.actor_id.as_deref()) {
        (Some(kind), Some(id)) => Some(actor_ref(kind, id)?),
        (None, None) => None,
        _ => return Err(internal("incomplete persisted notification actor")),
    };
    let subject = match (row.entry_id, details.name, details.path, details.entry_type) {
        (Some(entry_id), Some(name), Some(path), Some(kind)) => Some(EntrySnapshot {
            entry_id: EntryId::from_uuid(entry_id)
                .map_err(|_| internal("invalid persisted notification entry identifier"))?,
            name,
            path: EntryPath::new(path)
                .map_err(|_| internal("invalid persisted notification path"))?,
            kind: entry_kind(&kind)?,
        }),
        _ => None,
    };
    Ok(Notification {
        id: NotificationId::from_uuid(row.notification_id)
            .map_err(|_| internal("invalid persisted notification identifier"))?,
        kind: decode_kind(&row.kind)?,
        actor,
        subject,
        access: details.access_mask.map(decode_access).transpose()?,
        access_request_id: details
            .access_request_id
            .map(AccessRequestId::from_uuid)
            .transpose()
            .map_err(|_| internal("invalid persisted access-request identifier"))?,
        decision: details.decision,
        created_at: row.created_at,
        read_at: row.read_at,
    })
}

const fn encode_kind(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::AccessGranted => "access_granted",
        NotificationKind::AccessRevoked => "access_revoked",
        NotificationKind::AccessRequested => "access_requested",
        NotificationKind::AccessRequestDecided => "access_request_decided",
    }
}

fn decode_kind(value: &str) -> Result<NotificationKind> {
    match value {
        "access_granted" => Ok(NotificationKind::AccessGranted),
        "access_revoked" => Ok(NotificationKind::AccessRevoked),
        "access_requested" => Ok(NotificationKind::AccessRequested),
        "access_request_decided" => Ok(NotificationKind::AccessRequestDecided),
        _ => Err(internal("invalid persisted notification kind")),
    }
}

const fn encode_entry_kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Folder => "folder",
    }
}
