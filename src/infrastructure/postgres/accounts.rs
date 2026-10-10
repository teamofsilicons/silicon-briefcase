//! The local cache of Silicon Accounts accounts, their revocations, and the
//! webhook deliveries already applied.
//!
//! Rows are keyed by the account uuid. Every field that can change remembers
//! when it was observed, so an older sighting (a stale token, a late webhook)
//! never overwrites a newer one: Silicon Accounts does not guarantee webhook
//! order.

use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;

use crate::domain::actor::ActorKind;

/// Whether Briefcase may serve an account.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountStatus {
    /// Signed in, or at least not signed out everywhere.
    Active,
    /// The account removed Briefcase's access; a new sign-in reactivates it.
    AccessRemoved,
    /// The account was deleted at Silicon Accounts.
    Deleted,
}

impl AccountStatus {
    fn parse(value: &str) -> Result<Self, sqlx::Error> {
        match value {
            "active" => Ok(Self::Active),
            "access_removed" => Ok(Self::AccessRemoved),
            "deleted" => Ok(Self::Deleted),
            other => Err(sqlx::Error::Decode(
                format!("unknown account status `{other}`").into(),
            )),
        }
    }
}

/// One cached account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountRecord {
    /// Permanent account uuid.
    pub uuid: String,
    /// Carbon or Silicon.
    pub kind: ActorKind,
    /// Current public id, when known.
    pub public_id: Option<String>,
    /// When the public id was observed.
    pub public_id_observed_at: Option<OffsetDateTime>,
    /// Display name, when known.
    pub display_name: Option<String>,
    /// Profile photo URL, when known.
    pub pfp_url: Option<String>,
    /// Email the account shared with Briefcase.
    pub email: Option<String>,
    /// A Silicon's custodian, when known.
    pub custodian_uuid: Option<String>,
    /// Whether Briefcase may serve the account.
    pub status: AccountStatus,
    /// Access tokens issued at or before this instant are refused.
    pub revoked_before: Option<OffsetDateTime>,
    /// Last confirmation by a lookup or webhook.
    pub refreshed_at: OffsetDateTime,
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    account_uuid: String,
    kind: String,
    public_id: Option<String>,
    public_id_observed_at: Option<OffsetDateTime>,
    display_name: Option<String>,
    pfp_url: Option<String>,
    email: Option<String>,
    custodian_uuid: Option<String>,
    status: String,
    revoked_before: Option<OffsetDateTime>,
    refreshed_at: OffsetDateTime,
}

impl TryFrom<AccountRow> for AccountRecord {
    type Error = sqlx::Error;

    fn try_from(row: AccountRow) -> Result<Self, Self::Error> {
        Ok(Self {
            kind: parse_kind(&row.kind)?,
            status: AccountStatus::parse(&row.status)?,
            uuid: row.account_uuid,
            public_id: row.public_id,
            public_id_observed_at: row.public_id_observed_at,
            display_name: row.display_name,
            pfp_url: row.pfp_url,
            email: row.email,
            custodian_uuid: row.custodian_uuid,
            revoked_before: row.revoked_before,
            refreshed_at: row.refreshed_at,
        })
    }
}

macro_rules! account_columns {
    () => {
        "account_uuid, kind, public_id, public_id_observed_at, display_name, pfp_url, email, \
         custodian_uuid, status, revoked_before, refreshed_at"
    };
}

/// A verified sighting of an account from an access token.
#[derive(Clone, Copy, Debug)]
pub struct TokenSighting<'a> {
    /// The token's `sub`.
    pub uuid: &'a str,
    /// The token's `kind`.
    pub kind: ActorKind,
    /// The token's `id`, as it was when the token was issued.
    pub public_id: Option<&'a str>,
    /// The token's `iat`.
    pub issued_at: OffsetDateTime,
}

/// What Silicon Accounts said about an account just now.
#[derive(Clone, Debug)]
pub struct AccountSnapshot {
    /// Permanent uuid.
    pub uuid: String,
    /// Carbon or Silicon.
    pub kind: ActorKind,
    /// Current public id (empty for deleted accounts).
    pub public_id: Option<String>,
    /// Display name.
    pub display_name: Option<String>,
    /// Profile photo URL.
    pub pfp_url: Option<String>,
    /// Email, when the snapshot came from the account's own token and it
    /// shared one. `None` leaves the cached email unchanged.
    pub email: Option<Option<String>>,
    /// A Silicon's custodian.
    pub custodian_uuid: Option<String>,
    /// Silicon Accounts status (`active`, `unclaimed`, `pending_custodian`, `deleted`).
    pub remote_status: String,
    /// Profile version, when known.
    pub version: Option<i64>,
}

/// Outcome of one webhook delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebhookOutcome {
    /// The event changed the cache.
    Applied,
    /// A newer state was already cached; nothing changed.
    Stale,
    /// The event needs no action (ping, unknown type, own sign-out).
    Ignored,
    /// The event id was applied before.
    Duplicate,
}

impl WebhookOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Stale => "stale",
            Self::Ignored | Self::Duplicate => "ignored",
        }
    }
}

/// One change a webhook asks for, already decoded and validated.
#[derive(Clone, Debug)]
pub enum AccountChange {
    /// `account.id_changed`.
    IdChanged {
        /// The account's kind, when the event carried it.
        kind: Option<ActorKind>,
        /// The id it had.
        old_id: String,
        /// The id it has now.
        new_id: String,
    },
    /// `account.updated`.
    Updated(Box<AccountSnapshot>),
    /// `silicon.custodian_changed`.
    CustodianChanged {
        /// The new custodian.
        custodian_uuid: String,
    },
    /// `membership.signed_out` for a reason that ends every sign-in.
    SignedOut,
    /// `membership.access_removed`.
    AccessRemoved,
    /// `account.deleted`.
    Deleted,
    /// Nothing to do (`ping`, an app's own `app_revoked` sign-out, unknown types).
    Ignore,
}

/// One webhook delivery to apply.
#[derive(Clone, Debug)]
pub struct WebhookDelivery {
    /// Dedupe key.
    pub event_id: String,
    /// Event type as delivered.
    pub event_type: String,
    /// When the change happened at Silicon Accounts.
    pub occurred_at: OffsetDateTime,
    /// The account the event is about, when any.
    pub account_uuid: Option<String>,
    /// The decoded change.
    pub change: AccountChange,
}

/// The account cache in PostgreSQL.
#[derive(Clone, Debug)]
pub struct AccountStore {
    pool: PgPool,
}

impl AccountStore {
    /// Wraps the API pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Loads one account.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn find(&self, uuid: &str) -> Result<Option<AccountRecord>, sqlx::Error> {
        sqlx::query_as::<_, AccountRow>(concat!(
            "SELECT ",
            account_columns!(),
            " FROM briefcase.accounts WHERE account_uuid = $1"
        ))
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?
        .map(AccountRecord::try_from)
        .transpose()
    }

    /// Loads the live accounts currently holding a public id (normally one).
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn find_by_public_id(
        &self,
        public_id: &str,
    ) -> Result<Vec<AccountRecord>, sqlx::Error> {
        sqlx::query_as::<_, AccountRow>(concat!(
            "SELECT ",
            account_columns!(),
            " FROM briefcase.accounts \
              WHERE public_id = $1 AND status <> 'deleted' \
              ORDER BY public_id_observed_at DESC NULLS LAST, account_uuid"
        ))
        .bind(public_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(AccountRecord::try_from)
        .collect()
    }

    /// Lists the cached Silicons whose custodian is `custodian_uuid`.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn custodial_silicons(
        &self,
        custodian_uuid: &str,
    ) -> Result<Vec<AccountRecord>, sqlx::Error> {
        sqlx::query_as::<_, AccountRow>(concat!(
            "SELECT ",
            account_columns!(),
            " FROM briefcase.accounts \
              WHERE custodian_uuid = $1 AND kind = 'silicon' AND status <> 'deleted' \
              ORDER BY public_id, account_uuid"
        ))
        .bind(custodian_uuid)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(AccountRecord::try_from)
        .collect()
    }

    /// Records a verified access token. Writes only when the token tells
    /// Briefcase something new: a first sighting, a newer public id, or a
    /// sign-in after the account had removed Briefcase's access.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn observe_token(
        &self,
        sighting: TokenSighting<'_>,
    ) -> Result<AccountRecord, sqlx::Error> {
        if let Some(record) = self.find(sighting.uuid).await?
            && !token_changes(&record, &sighting)
        {
            return Ok(record);
        }
        let row = sqlx::query_as::<_, AccountRow>(concat!(
            "INSERT INTO briefcase.accounts AS account \
                 (account_uuid, kind, public_id, public_id_observed_at) \
             VALUES ($1, $2, $3, CASE WHEN $3::text IS NULL THEN NULL ELSE $4::timestamptz END) \
             ON CONFLICT (account_uuid) DO UPDATE SET \
                 kind = EXCLUDED.kind, \
                 public_id = CASE \
                     WHEN EXCLUDED.public_id IS NOT NULL \
                      AND (account.public_id_observed_at IS NULL \
                           OR account.public_id_observed_at < EXCLUDED.public_id_observed_at) \
                     THEN EXCLUDED.public_id ELSE account.public_id END, \
                 public_id_observed_at = CASE \
                     WHEN EXCLUDED.public_id IS NOT NULL \
                      AND (account.public_id_observed_at IS NULL \
                           OR account.public_id_observed_at < EXCLUDED.public_id_observed_at) \
                     THEN EXCLUDED.public_id_observed_at ELSE account.public_id_observed_at END, \
                 status = CASE \
                     WHEN account.status = 'access_removed' \
                      AND (account.revoked_before IS NULL OR account.revoked_before < $4) \
                     THEN 'active' ELSE account.status END \
             RETURNING ",
            account_columns!()
        ))
        .bind(sighting.uuid)
        .bind(kind_name(sighting.kind))
        .bind(sighting.public_id.filter(|id| valid_public_id(id)))
        .bind(sighting.issued_at)
        .fetch_one(&self.pool)
        .await?;
        AccountRecord::try_from(row)
    }

    /// Records an account another app acts for (from a verified proof),
    /// without touching its sign-in state.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn observe_account(
        &self,
        uuid: &str,
        kind: ActorKind,
        public_id: Option<&str>,
    ) -> Result<AccountRecord, sqlx::Error> {
        if let Some(record) = self.find(uuid).await? {
            return Ok(record);
        }
        let row = sqlx::query_as::<_, AccountRow>(concat!(
            "INSERT INTO briefcase.accounts AS account \
                 (account_uuid, kind, public_id, public_id_observed_at) \
             VALUES ($1, $2, $3, CASE WHEN $3::text IS NULL THEN NULL ELSE clock_timestamp() END) \
             ON CONFLICT (account_uuid) DO UPDATE SET kind = account.kind \
             RETURNING ",
            account_columns!()
        ))
        .bind(uuid)
        .bind(kind_name(kind))
        .bind(public_id.filter(|id| valid_public_id(id)))
        .fetch_one(&self.pool)
        .await?;
        AccountRecord::try_from(row)
    }

    /// Stores what Silicon Accounts said just now (a lookup, or the account's
    /// own profile). Newer webhooks win over this only when they are newer.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn apply_snapshot(
        &self,
        snapshot: &AccountSnapshot,
    ) -> Result<AccountRecord, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let now = database_now(&mut transaction).await?;
        let record = upsert_snapshot(&mut transaction, snapshot, now).await?;
        if snapshot.remote_status == "deleted" {
            mark_deleted(&mut transaction, &snapshot.uuid, snapshot.kind, now).await?;
        }
        let record = match snapshot.remote_status.as_str() {
            "deleted" => load(&mut transaction, &snapshot.uuid)
                .await?
                .unwrap_or(record),
            _ => record,
        };
        transaction.commit().await?;
        Ok(record)
    }

    /// Returns whether a Silicon (or its custodian) allowed `account` to share
    /// with it.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn share_allowed(&self, silicon: &str, account: &str) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM briefcase.silicon_share_allowances \
                             WHERE silicon_uuid = $1 AND allowed_uuid = $2)",
        )
        .bind(silicon)
        .bind(account)
        .fetch_one(&self.pool)
        .await
    }

    /// Lets `account` share with a Silicon. Returns whether it was new.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn allow_sharer(
        &self,
        silicon: &str,
        account: &str,
        allowed_by: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(
            "INSERT INTO briefcase.silicon_share_allowances \
                 (silicon_uuid, allowed_uuid, allowed_by_uuid) \
             VALUES ($1, $2, $3) ON CONFLICT (silicon_uuid, allowed_uuid) DO NOTHING",
        )
        .bind(silicon)
        .bind(account)
        .bind(allowed_by)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// Stops letting `account` share with a Silicon. Returns whether it had
    /// been allowed. Shares it already made stay.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn disallow_sharer(&self, silicon: &str, account: &str) -> Result<bool, sqlx::Error> {
        Ok(sqlx::query(
            "DELETE FROM briefcase.silicon_share_allowances \
              WHERE silicon_uuid = $1 AND allowed_uuid = $2",
        )
        .bind(silicon)
        .bind(account)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1)
    }

    /// Lists the accounts a Silicon lets share with it, oldest first.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn allowed_sharers(
        &self,
        silicon: &str,
    ) -> Result<Vec<(AccountRecord, OffsetDateTime)>, sqlx::Error> {
        #[derive(sqlx::FromRow)]
        struct AllowedRow {
            #[sqlx(flatten)]
            account: AccountRow,
            allowed_at: OffsetDateTime,
        }
        sqlx::query_as::<_, AllowedRow>(
            "SELECT account.account_uuid, account.kind, account.public_id, \
                    account.public_id_observed_at, account.display_name, account.pfp_url, \
                    account.email, account.custodian_uuid, account.status, \
                    account.revoked_before, account.refreshed_at, \
                    allowance.created_at AS allowed_at \
               FROM briefcase.silicon_share_allowances AS allowance \
               JOIN briefcase.accounts AS account ON account.account_uuid = allowance.allowed_uuid \
              WHERE allowance.silicon_uuid = $1 \
              ORDER BY allowance.created_at, account.account_uuid",
        )
        .bind(silicon)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| Ok((AccountRecord::try_from(row.account)?, row.allowed_at)))
        .collect()
    }

    /// Applies one webhook delivery exactly once.
    ///
    /// # Errors
    ///
    /// Returns the database error; the delivery is then retried by Silicon
    /// Accounts.
    pub async fn apply_webhook(
        &self,
        delivery: &WebhookDelivery,
    ) -> Result<WebhookOutcome, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let seen = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM briefcase.accounts_webhook_receipts WHERE event_id = $1)",
        )
        .bind(&delivery.event_id)
        .fetch_one(&mut *transaction)
        .await?;
        if seen {
            transaction.rollback().await?;
            return Ok(WebhookOutcome::Duplicate);
        }
        let outcome = match &delivery.account_uuid {
            Some(uuid) => apply_change(&mut transaction, uuid, delivery).await?,
            None => WebhookOutcome::Ignored,
        };
        let recorded = sqlx::query_scalar::<_, String>(
            "INSERT INTO briefcase.accounts_webhook_receipts \
                 (event_id, event_type, account_uuid, occurred_at, outcome) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (event_id) DO NOTHING RETURNING event_id",
        )
        .bind(&delivery.event_id)
        .bind(&delivery.event_type)
        .bind(delivery.account_uuid.as_deref())
        .bind(delivery.occurred_at)
        .bind(outcome.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        if recorded.is_none() {
            // A concurrent delivery of the same event won the race.
            transaction.rollback().await?;
            return Ok(WebhookOutcome::Duplicate);
        }
        transaction.commit().await?;
        Ok(outcome)
    }
}

fn token_changes(record: &AccountRecord, sighting: &TokenSighting<'_>) -> bool {
    let newer_id = sighting.public_id.is_some_and(|id| {
        valid_public_id(id)
            && record.public_id.as_deref() != Some(id)
            && record
                .public_id_observed_at
                .is_none_or(|observed| observed < sighting.issued_at)
    });
    let reactivates = record.status == AccountStatus::AccessRemoved
        && record
            .revoked_before
            .is_none_or(|revoked| revoked < sighting.issued_at);
    newer_id || reactivates || record.kind != sighting.kind
}

#[allow(clippy::too_many_lines)]
async fn apply_change(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
    delivery: &WebhookDelivery,
) -> Result<WebhookOutcome, sqlx::Error> {
    let at = delivery.occurred_at;
    let changed = match &delivery.change {
        AccountChange::Ignore => return Ok(WebhookOutcome::Ignored),
        AccountChange::IdChanged {
            kind,
            old_id,
            new_id,
        } => {
            let kind = kind.unwrap_or_else(|| kind_of_public_id(new_id));
            ensure_account(transaction, uuid, kind).await?;
            if valid_public_id(old_id) {
                sqlx::query(
                    "INSERT INTO briefcase.account_id_history (public_id, account_uuid, replaced_at) \
                     VALUES ($1, $2, $3) \
                     ON CONFLICT (public_id, account_uuid) \
                     DO UPDATE SET replaced_at = GREATEST(account_id_history.replaced_at, EXCLUDED.replaced_at)",
                )
                .bind(old_id)
                .bind(uuid)
                .bind(at)
                .execute(&mut **transaction)
                .await?;
            }
            if !valid_public_id(new_id) {
                return Ok(WebhookOutcome::Ignored);
            }
            sqlx::query(
                "UPDATE briefcase.accounts SET public_id = $2, public_id_observed_at = $3, \
                        refreshed_at = clock_timestamp() \
                  WHERE account_uuid = $1 \
                    AND (public_id_observed_at IS NULL OR public_id_observed_at < $3)",
            )
            .bind(uuid)
            .bind(new_id)
            .bind(at)
            .execute(&mut **transaction)
            .await?
            .rows_affected()
        }
        AccountChange::Updated(snapshot) => {
            ensure_account(transaction, uuid, snapshot.kind).await?;
            let mut rows = sqlx::query(
                "UPDATE briefcase.accounts SET \
                        display_name = $2, pfp_url = $3, \
                        email = CASE WHEN $4 THEN $5 ELSE email END, \
                        profile_version = COALESCE($6, profile_version), \
                        refreshed_at = clock_timestamp() \
                  WHERE account_uuid = $1 AND status <> 'deleted' \
                    AND ($6::bigint IS NULL OR profile_version < $6)",
            )
            .bind(uuid)
            .bind(snapshot.display_name.as_deref())
            .bind(snapshot.pfp_url.as_deref())
            .bind(snapshot.email.is_some())
            .bind(snapshot.email.clone().flatten())
            .bind(snapshot.version)
            .execute(&mut **transaction)
            .await?
            .rows_affected();
            if let Some(public_id) = snapshot
                .public_id
                .as_deref()
                .filter(|id| valid_public_id(id))
            {
                rows += sqlx::query(
                    "UPDATE briefcase.accounts SET public_id = $2, public_id_observed_at = $3 \
                      WHERE account_uuid = $1 AND status <> 'deleted' \
                        AND (public_id_observed_at IS NULL OR public_id_observed_at < $3)",
                )
                .bind(uuid)
                .bind(public_id)
                .bind(at)
                .execute(&mut **transaction)
                .await?
                .rows_affected();
            }
            rows
        }
        AccountChange::CustodianChanged { custodian_uuid } => {
            ensure_account(transaction, uuid, ActorKind::Silicon).await?;
            sqlx::query(
                "UPDATE briefcase.accounts SET custodian_uuid = $2, custodian_observed_at = $3, \
                        refreshed_at = clock_timestamp() \
                  WHERE account_uuid = $1 AND kind = 'silicon' \
                    AND (custodian_observed_at IS NULL OR custodian_observed_at < $3)",
            )
            .bind(uuid)
            .bind(custodian_uuid)
            .bind(at)
            .execute(&mut **transaction)
            .await?
            .rows_affected()
        }
        AccountChange::SignedOut => revoke(transaction, uuid, at, false).await?,
        AccountChange::AccessRemoved => revoke(transaction, uuid, at, true).await?,
        AccountChange::Deleted => {
            let kind = load(transaction, uuid)
                .await?
                .map_or(ActorKind::Carbon, |record| record.kind);
            let changed = mark_deleted(transaction, uuid, kind, at).await?;
            // Idempotent, and run for a repeated deletion too, in the same
            // transaction as the receipt: a failure is retried as a whole.
            erase_deleted_account(transaction, uuid, kind, &delivery.event_id).await?;
            changed
        }
    };
    Ok(if changed > 0 {
        WebhookOutcome::Applied
    } else {
        WebhookOutcome::Stale
    })
}

/// Refuses every access token issued at or before `at`; with
/// `access_removed`, also marks the account until it signs in again.
async fn revoke(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
    at: OffsetDateTime,
    access_removed: bool,
) -> Result<u64, sqlx::Error> {
    // Revocation must hold even for an account Briefcase never saw: insert a
    // placeholder whose kind is corrected by the first verified sign-in.
    sqlx::query(
        "INSERT INTO briefcase.accounts (account_uuid, kind, refreshed_at) \
         VALUES ($1, 'carbon', clock_timestamp()) ON CONFLICT (account_uuid) DO NOTHING",
    )
    .bind(uuid)
    .execute(&mut **transaction)
    .await?;
    Ok(sqlx::query(
        "UPDATE briefcase.accounts SET \
                revoked_before = GREATEST(COALESCE(revoked_before, $2), $2), \
                status = CASE WHEN $3 AND status = 'active' THEN 'access_removed' ELSE status END \
          WHERE account_uuid = $1 \
            AND (revoked_before IS NULL OR revoked_before < $2 \
                 OR ($3 AND status = 'active'))",
    )
    .bind(uuid)
    .bind(at)
    .bind(access_removed)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

/// `account.deleted`: everything in the account's own drive is deleted
/// permanently (the worker purges it, objects in a bring-your-own bucket
/// included), every live share to it ends in every drive, and it leaves every
/// Silicon allow-list. What it created in other accounts' drives belongs to
/// those drives and stays.
async fn erase_deleted_account(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
    kind: ActorKind,
    event_id: &str,
) -> Result<(), sqlx::Error> {
    let request_id = format!(
        "accounts-webhook:{}",
        event_id.chars().take(200).collect::<String>()
    );
    let erased =
        sqlx::query_scalar::<_, i64>("SELECT briefcase.erase_deleted_account_drive($1, $2)")
            .bind(uuid)
            .bind(&request_id)
            .fetch_one(&mut **transaction)
            .await?;
    let revoked =
        sqlx::query_scalar::<_, i64>("SELECT briefcase.revoke_deleted_account_shares($1, $2, $3)")
            .bind(kind_name(kind))
            .bind(uuid)
            .bind(&request_id)
            .fetch_one(&mut **transaction)
            .await?;
    sqlx::query(
        "DELETE FROM briefcase.silicon_share_allowances \
          WHERE silicon_uuid = $1 OR allowed_uuid = $1",
    )
    .bind(uuid)
    .execute(&mut **transaction)
    .await?;
    tracing::info!(
        event = "deleted_account_erased",
        entries = erased,
        shares_ended = revoked,
        "a deleted account's drive was erased and its shares ended"
    );
    Ok(())
}

/// Marks an account deleted and forgets what identified it. Its drive is
/// erased by [`erase_deleted_account`] in the same transaction.
async fn mark_deleted(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
    kind: ActorKind,
    at: OffsetDateTime,
) -> Result<u64, sqlx::Error> {
    ensure_account(transaction, uuid, kind).await?;
    Ok(sqlx::query(
        "UPDATE briefcase.accounts SET status = 'deleted', deleted_at = $2, \
                revoked_before = GREATEST(COALESCE(revoked_before, $2), $2), \
                display_name = NULL, pfp_url = NULL, email = NULL \
          WHERE account_uuid = $1 AND status <> 'deleted'",
    )
    .bind(uuid)
    .bind(at)
    .execute(&mut **transaction)
    .await?
    .rows_affected())
}

async fn ensure_account(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
    kind: ActorKind,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO briefcase.accounts (account_uuid, kind) VALUES ($1, $2) \
         ON CONFLICT (account_uuid) DO NOTHING",
    )
    .bind(uuid)
    .bind(kind_name(kind))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn upsert_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    snapshot: &AccountSnapshot,
    now: OffsetDateTime,
) -> Result<AccountRecord, sqlx::Error> {
    let custodian = match snapshot.kind {
        ActorKind::Silicon => snapshot.custodian_uuid.as_deref(),
        ActorKind::Carbon => None,
    };
    let row = sqlx::query_as::<_, AccountRow>(concat!(
        "INSERT INTO briefcase.accounts AS account \
             (account_uuid, kind, public_id, public_id_observed_at, display_name, pfp_url, \
              email, custodian_uuid, custodian_observed_at, profile_version, refreshed_at) \
         VALUES ($1, $2, $3, CASE WHEN $3::text IS NULL THEN NULL ELSE $9 END, $4, $5, \
                 CASE WHEN $6 THEN $7 ELSE NULL END, $8, \
                 CASE WHEN $8::text IS NULL THEN NULL ELSE $9 END, COALESCE($10, 0), $9) \
         ON CONFLICT (account_uuid) DO UPDATE SET \
             public_id = COALESCE(EXCLUDED.public_id, account.public_id), \
             public_id_observed_at = COALESCE(EXCLUDED.public_id_observed_at, account.public_id_observed_at), \
             display_name = EXCLUDED.display_name, \
             pfp_url = EXCLUDED.pfp_url, \
             email = CASE WHEN $6 THEN EXCLUDED.email ELSE account.email END, \
             custodian_uuid = COALESCE(EXCLUDED.custodian_uuid, account.custodian_uuid), \
             custodian_observed_at = COALESCE(EXCLUDED.custodian_observed_at, account.custodian_observed_at), \
             profile_version = GREATEST(account.profile_version, EXCLUDED.profile_version), \
             refreshed_at = EXCLUDED.refreshed_at \
         RETURNING ", account_columns!()
    ))
    .bind(&snapshot.uuid)
    .bind(kind_name(snapshot.kind))
    .bind(snapshot.public_id.as_deref().filter(|id| valid_public_id(id)))
    .bind(snapshot.display_name.as_deref().filter(|name| name.len() <= 512))
    .bind(snapshot.pfp_url.as_deref().filter(|url| url.len() <= 2048))
    .bind(snapshot.email.is_some())
    .bind(
        snapshot
            .email
            .clone()
            .flatten()
            .filter(|email| (3..=320).contains(&email.len())),
    )
    .bind(custodian)
    .bind(now)
    .bind(snapshot.version)
    .fetch_one(&mut **transaction)
    .await?;
    AccountRecord::try_from(row)
}

async fn load(
    transaction: &mut Transaction<'_, Postgres>,
    uuid: &str,
) -> Result<Option<AccountRecord>, sqlx::Error> {
    sqlx::query_as::<_, AccountRow>(concat!(
        "SELECT ",
        account_columns!(),
        " FROM briefcase.accounts WHERE account_uuid = $1"
    ))
    .bind(uuid)
    .fetch_optional(&mut **transaction)
    .await?
    .map(AccountRecord::try_from)
    .transpose()
}

async fn database_now(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<OffsetDateTime, sqlx::Error> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await
}

const fn kind_name(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Carbon => "carbon",
        ActorKind::Silicon => "silicon",
    }
}

fn kind_of_public_id(id: &str) -> ActorKind {
    if id.starts_with("si:") {
        ActorKind::Silicon
    } else {
        ActorKind::Carbon
    }
}

fn parse_kind(value: &str) -> Result<ActorKind, sqlx::Error> {
    match value {
        "carbon" => Ok(ActorKind::Carbon),
        "silicon" => Ok(ActorKind::Silicon),
        other => Err(sqlx::Error::Decode(
            format!("unknown account kind `{other}`").into(),
        )),
    }
}

fn valid_public_id(id: &str) -> bool {
    crate::domain::account::PublicId::parse(id).is_ok_and(|parsed| parsed.as_str() == id)
}
