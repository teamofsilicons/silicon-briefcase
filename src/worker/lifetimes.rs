//! Expiring-share expiry records and self-destruct deletion.
//!
//! Expiring shares stop working the instant they expire because every access
//! decision filters on `expires_at`; this sweep only writes the expiry into the
//! entry's log (and clears an expired expiring link) without notifying anyone.
//! Self-destructing files are deleted here, permanently: each becomes its own
//! deletion batch that is already due, so object cleanup purges it at once
//! and the file never appears in a bin.
//!
//! Every statement claims rows with `SKIP LOCKED`, so concurrent workers never
//! record the same expiry twice.

use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LifetimeStats {
    pub(crate) self_destructed: u64,
    pub(crate) expiring_shares_expired: u64,
    pub(crate) expiring_links_expired: u64,
}

impl LifetimeStats {
    pub(crate) const fn is_empty(self) -> bool {
        self.self_destructed == 0
            && self.expiring_shares_expired == 0
            && self.expiring_links_expired == 0
    }
}

pub(crate) async fn sweep(pool: &PgPool, batch_size: i64) -> Result<LifetimeStats, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let mut counts = [0_u64; 4];
    for (count, statement) in counts.iter_mut().zip([
        SELF_DESTRUCT,
        MEMBER_EXPIRING_EXPIRY,
        TAG_EXPIRING_EXPIRY,
        LINK_EXPIRING_EXPIRY,
    ]) {
        *count = sqlx::query(statement)
            .bind(batch_size)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
    }
    transaction.commit().await?;
    let [
        self_destructed,
        member_shares,
        tag_shares,
        expiring_links_expired,
    ] = counts;
    Ok(LifetimeStats {
        self_destructed,
        expiring_shares_expired: member_shares + tag_shares,
        expiring_links_expired,
    })
}

/// Deletes due self-destructing files for good. The log attributes the
/// deletion to the file's creator, who chose the timer; `origin_app_id` stays
/// null because no application acted.
const SELF_DESTRUCT: &str = "WITH due AS ( \
             SELECT org_id, entry_id, self_destruct_at FROM briefcase.entries \
              WHERE self_destruct_at <= clock_timestamp() AND deleted_at IS NULL \
              ORDER BY self_destruct_at LIMIT $1 FOR UPDATE SKIP LOCKED \
         ), deleted AS ( \
             UPDATE briefcase.entries AS entry \
                SET deletion_batch_id = briefcase.new_uuid_v7(), deleted_at = clock_timestamp(), \
                    purge_after = clock_timestamp() \
               FROM due \
              WHERE entry.org_id = due.org_id AND entry.entry_id = due.entry_id \
             RETURNING entry.org_id, entry.entry_id, entry.created_by_type, entry.created_by_id, \
                       due.self_destruct_at \
         ) \
         INSERT INTO briefcase.audit_events \
                (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata, occurred_at) \
         SELECT org_id, briefcase.new_uuid_v7(), entry_id, created_by_type, created_by_id, \
                'entry.self_destructed.v1', 'worker:self-destruct', \
                jsonb_build_object('self_destruct_at', self_destruct_at, 'automatic', true), \
                clock_timestamp() \
           FROM deleted";

/// Logs expired member expiring shares at the instant each ended, by the member
/// who shared it, so the log reads the same however late the sweep runs.
const MEMBER_EXPIRING_EXPIRY: &str = "WITH due AS ( \
             SELECT org_id, grant_id FROM briefcase.permission_grants \
              WHERE expires_at <= clock_timestamp() AND expiry_logged_at IS NULL AND revoked_at IS NULL \
              ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED \
         ), marked AS ( \
             UPDATE briefcase.permission_grants AS grant_row SET expiry_logged_at = clock_timestamp() \
               FROM due WHERE grant_row.org_id = due.org_id AND grant_row.grant_id = due.grant_id \
             RETURNING grant_row.org_id, grant_row.entry_id, grant_row.grant_id, grant_row.principal_type, \
                       grant_row.principal_id, grant_row.granted_by_type, grant_row.granted_by_id, \
                       grant_row.expires_at \
         ) \
         INSERT INTO briefcase.audit_events \
                (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata, occurred_at) \
         SELECT org_id, briefcase.new_uuid_v7(), entry_id, granted_by_type, granted_by_id, \
                'permission.expiring_share_expired.v1', 'worker:share-expiry', \
                jsonb_build_object('grant_id', grant_id, 'expires_at', expires_at, 'automatic', true, \
                                   'principal', jsonb_build_object('type', principal_type, 'id', principal_id)), \
                expires_at \
           FROM marked";

/// Logs expired tag expiring shares the same way.
const TAG_EXPIRING_EXPIRY: &str = "WITH due AS ( \
             SELECT org_id, grant_id FROM briefcase.tag_permission_grants \
              WHERE expires_at <= clock_timestamp() AND expiry_logged_at IS NULL AND revoked_at IS NULL \
              ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED \
         ), marked AS ( \
             UPDATE briefcase.tag_permission_grants AS grant_row SET expiry_logged_at = clock_timestamp() \
               FROM due WHERE grant_row.org_id = due.org_id AND grant_row.grant_id = due.grant_id \
             RETURNING grant_row.org_id, grant_row.entry_id, grant_row.grant_id, grant_row.tag_id, \
                       grant_row.granted_by_type, grant_row.granted_by_id, grant_row.expires_at \
         ) \
         INSERT INTO briefcase.audit_events \
                (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata, occurred_at) \
         SELECT org_id, briefcase.new_uuid_v7(), entry_id, granted_by_type, granted_by_id, \
                'permission.tag_expiring_share_expired.v1', 'worker:share-expiry', \
                jsonb_build_object('grant_id', grant_id, 'tag_id', tag_id, 'expires_at', expires_at, \
                                   'automatic', true), \
                expires_at \
           FROM marked";

/// Clears expired expiring links. They are already off for readers; clearing
/// keeps the stored setting honest. Nobody recorded who enabled a link, so the
/// log attributes the expiry to the entry's owner.
const LINK_EXPIRING_EXPIRY: &str = "WITH due AS ( \
             SELECT org_id, entry_id, link_expires_at FROM briefcase.entries \
              WHERE link_expires_at <= clock_timestamp() \
              ORDER BY link_expires_at LIMIT $1 FOR UPDATE SKIP LOCKED \
         ), cleared AS ( \
             UPDATE briefcase.entries AS entry SET link_public = false, link_expires_at = NULL \
               FROM due WHERE entry.org_id = due.org_id AND entry.entry_id = due.entry_id \
             RETURNING entry.org_id, entry.entry_id, entry.owner_type, entry.owner_id, due.link_expires_at \
         ) \
         INSERT INTO briefcase.audit_events \
                (org_id, audit_id, entry_id, actor_type, actor_id, action, request_id, metadata, occurred_at) \
         SELECT org_id, briefcase.new_uuid_v7(), entry_id, owner_type, owner_id, \
                'entry.expiring_link_expired.v1', 'worker:share-expiry', \
                jsonb_build_object('expires_at', link_expires_at, 'automatic', true), \
                link_expires_at \
           FROM cleared";
