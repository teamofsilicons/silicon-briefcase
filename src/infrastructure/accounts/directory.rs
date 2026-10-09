//! The account cache and the custodian rule, backed by Silicon Accounts.
//!
//! Every authenticated request passes through here: the credential is
//! verified, the account is recorded (or refused because its sign-ins ended),
//! and callers that name other accounts by `c:`/`si:` id get them resolved to
//! uuids. Custodian facts come from the cache, refreshed by lookups (budgeted)
//! and by `silicon.custodian_changed` webhooks.

use std::{sync::Arc, time::Duration};

use silicon_accounts_client::{
    AccountForApp, AccountKind, AccountSummary, ProofKind, ProofVerification, WebhookEvent,
    WebhookPayload,
};
use time::OffsetDateTime;

use super::{AccountsError, AccountsGateway, Introspection};
use crate::{
    config::{AccountsSettings, ProofIssuers},
    domain::{
        account::{
            AccountUuid, Actor, Credential, CustodyFacts, PublicId, is_account_uuid,
            is_custodian_of, same_circle,
        },
        actor::ActorKind,
    },
    error::AppError,
    infrastructure::postgres::accounts::{
        AccountChange, AccountRecord, AccountSnapshot, AccountStatus, AccountStore, TokenSighting,
        WebhookDelivery, WebhookOutcome,
    },
};

/// Verified identities, the account cache, and the custodian rule.
pub struct AccountDirectory {
    gateway: Arc<AccountsGateway>,
    store: AccountStore,
    ttl: Duration,
    proof_issuers: ProofIssuers,
}

impl std::fmt::Debug for AccountDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountDirectory")
            .field("gateway", &self.gateway)
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

impl AccountDirectory {
    /// Combines the gateway and the cache.
    #[must_use]
    pub fn new(
        gateway: Arc<AccountsGateway>,
        store: AccountStore,
        settings: &AccountsSettings,
    ) -> Self {
        Self {
            gateway,
            store,
            ttl: settings.account_cache_ttl,
            proof_issuers: settings.proof_issuers.clone(),
        }
    }

    /// The Silicon Accounts gateway.
    #[must_use]
    pub fn gateway(&self) -> &AccountsGateway {
        &self.gateway
    }

    /// The account cache.
    #[must_use]
    pub const fn store(&self) -> &AccountStore {
        &self.store
    }

    /// Verifies a Silicon Accounts access token and records the account.
    ///
    /// With `introspect`, Silicon Accounts is also asked whether the token is
    /// still active, so a revocation is seen at once.
    ///
    /// # Errors
    ///
    /// Returns a 401 with the exact reason for an invalid, revoked or deleted
    /// account's token, and 503 when Silicon Accounts cannot be reached for a
    /// check that needs it.
    pub async fn authenticate_token(
        &self,
        token: &str,
        introspect: bool,
    ) -> Result<Actor, AppError> {
        let claims = self.gateway.verify_access_token(token).await?;
        let uuid = AccountUuid::new(claims.sub.clone()).map_err(|_| {
            AppError::unauthorized(
                "token_invalid_subject",
                "The access token's `sub` is not a Silicon Accounts uuid. Sign in again with Silicon Accounts.",
            )
        })?;
        let kind = claims.kind.map(actor_kind).ok_or_else(|| {
            AppError::unauthorized(
                "token_missing_kind",
                "The access token has no `kind` claim, so Briefcase cannot tell a Carbon from a Silicon. Sign in again with Silicon Accounts.",
            )
        })?;
        let issued_at = claims
            .iat
            .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
            .ok_or_else(|| {
                AppError::unauthorized(
                    "token_missing_iat",
                    "The access token has no `iat` claim, so Briefcase cannot check it against sign-outs. Sign in again with Silicon Accounts.",
                )
            })?;
        if introspect {
            match self.gateway.introspect(token).await? {
                Introspection::Active { subject }
                    if subject
                        .as_deref()
                        .is_none_or(|subject| subject == uuid.as_str()) => {}
                _ => {
                    return Err(AppError::unauthorized(
                        "token_revoked",
                        "Silicon Accounts reports this access token is no longer active: the sign-in ended or the account removed Briefcase's access. Sign in again.",
                    ));
                }
            }
        }
        let record = self
            .store
            .observe_token(TokenSighting {
                uuid: uuid.as_str(),
                kind,
                public_id: claims.id.as_deref(),
                issued_at,
            })
            .await
            .map_err(database_error)?;
        refuse_ended(&record, issued_at)?;
        if kind == ActorKind::Silicon && record.custodian_uuid.is_none() {
            // Learn the custodian now, so the custodian rule has it; a failure
            // only delays that until the next lookup or webhook.
            if let Err(error) = self.refresh(&uuid).await {
                tracing::warn!(
                    event = "accounts_custodian_lookup_failed",
                    error = %error,
                    "could not look up a Silicon's custodian"
                );
            }
        }
        Ok(Actor {
            uuid,
            kind,
            id: claims.id,
            credential: Credential::AccessToken {
                token_family: claims.fid,
                membership_id: claims.mid,
                scope: claims.scope,
                issued_at: claims.iat,
            },
        })
    }

    /// Verifies a User verification proof another app sent for `scope`.
    ///
    /// # Errors
    ///
    /// Returns 401 `proof_invalid` for a proof Silicon Accounts does not
    /// confirm, 403 with the exact reason when the proof is valid but may not
    /// do this (wrong kind, missing scope, issuer not allowed, account
    /// deleted or its access removed), and 503 when the check failed.
    pub async fn authenticate_proof(&self, token: &str, scope: &str) -> Result<Actor, AppError> {
        let ProofVerification::Valid(proof) = self.gateway.verify_proof(token).await? else {
            return Err(invalid_proof());
        };
        if proof.receiving_app.app_id != self.gateway.app_id() {
            return Err(invalid_proof());
        }
        let issuer = proof.issuing_app.app_id.clone();
        if proof.kind != ProofKind::UserVerification {
            return Err(AppError::refused(
                "proof_kind_not_accepted",
                format!(
                    "Briefcase acts only for an account, but {issuer} sent an App verification proof. Send a User verification proof for the account."
                ),
            ));
        }
        if !proof.scopes.iter().any(|granted| granted == scope) {
            return Err(AppError::refused(
                "proof_scope_missing",
                format!(
                    "The proof from {issuer} does not carry the scope `{scope}` this route requires. Ask for a proof with that scope."
                ),
            ));
        }
        if !self.proof_issuers.allows(scope, &issuer) {
            return Err(AppError::refused(
                "proof_issuer_not_allowed",
                format!(
                    "{issuer} may not use `{scope}` at Briefcase. Briefcase's operator lists the apps allowed for each scope in BRIEFCASE_PROOF_ISSUERS."
                ),
            ));
        }
        let user = proof.user.as_ref().ok_or_else(invalid_proof)?;
        let uuid = AccountUuid::new(user.uuid.clone()).map_err(|_| invalid_proof())?;
        let record = match user.kind {
            Some(kind) => self
                .store
                .observe_account(uuid.as_str(), actor_kind(kind), Some(user.id.as_str()))
                .await
                .map_err(database_error)?,
            None => self.record(&uuid).await?,
        };
        match record.status {
            AccountStatus::Active => {}
            AccountStatus::Deleted => {
                return Err(AppError::refused(
                    "account_deleted",
                    "The account this proof acts for was deleted at Silicon Accounts.",
                ));
            }
            AccountStatus::AccessRemoved => {
                return Err(AppError::refused(
                    "account_access_removed",
                    format!(
                        "{} removed Briefcase's access, so no app can act for it here until it signs in to Briefcase again.",
                        display_id(&record)
                    ),
                ));
            }
        }
        Ok(Actor {
            uuid,
            kind: record.kind,
            id: (!user.id.is_empty()).then(|| user.id.clone()),
            credential: Credential::Proof {
                issuing_app: issuer,
                proof_id: proof.proof_id.clone(),
                scope: scope.to_owned(),
            },
        })
    }

    /// Returns the cached account, looking it up when Briefcase has never
    /// seen it.
    ///
    /// # Errors
    ///
    /// Returns 422 `unknown_account` when Silicon Accounts has no such
    /// account, or the lookup error.
    pub async fn record(&self, uuid: &AccountUuid) -> Result<AccountRecord, AppError> {
        if let Some(record) = self
            .store
            .find(uuid.as_str())
            .await
            .map_err(database_error)?
        {
            return Ok(record);
        }
        self.refresh(uuid)
            .await?
            .ok_or_else(|| unknown_account(uuid.as_str()))
    }

    /// Returns who an account is for the custodian rule.
    ///
    /// Carbons never change; a Silicon's custodian is refreshed by lookup when
    /// unknown or older than the cache lifetime (webhooks keep it current in
    /// between). A failed refresh falls back to the cached custodian.
    ///
    /// # Errors
    ///
    /// Returns 422 `unknown_account` for an account Silicon Accounts does not
    /// know, or the lookup error when nothing is cached.
    pub async fn custody(&self, uuid: &AccountUuid) -> Result<CustodyFacts, AppError> {
        let cached = self
            .store
            .find(uuid.as_str())
            .await
            .map_err(database_error)?;
        let fresh = cached.as_ref().is_some_and(|record| {
            record.kind == ActorKind::Carbon
                || (record.custodian_uuid.is_some() && self.is_fresh(record))
        });
        let record = if fresh {
            cached
        } else {
            match self.refresh(uuid).await {
                Ok(Some(record)) => Some(record),
                Ok(None) => cached,
                Err(error) if cached.is_some() => {
                    tracing::warn!(
                        event = "accounts_custody_refresh_failed",
                        error = %error,
                        "using the cached custodian"
                    );
                    cached
                }
                Err(error) => return Err(error),
            }
        };
        let record = record.ok_or_else(|| unknown_account(uuid.as_str()))?;
        Ok(CustodyFacts {
            uuid: uuid.clone(),
            kind: record.kind,
            custodian: record
                .custodian_uuid
                .and_then(|custodian| AccountUuid::new(custodian).ok()),
        })
    }

    /// Returns whether two accounts share a custodian circle.
    ///
    /// # Errors
    ///
    /// Propagates [`AccountDirectory::custody`] errors.
    pub async fn same_circle(
        &self,
        left: &AccountUuid,
        right: &AccountUuid,
    ) -> Result<bool, AppError> {
        if left == right {
            return Ok(true);
        }
        let (left, right) = (self.custody(left).await?, self.custody(right).await?);
        Ok(same_circle(&left, &right))
    }

    /// Returns whether `carbon` is the custodian of `silicon`.
    ///
    /// # Errors
    ///
    /// Propagates [`AccountDirectory::custody`] errors.
    pub async fn is_custodian_of(
        &self,
        carbon: &AccountUuid,
        silicon: &AccountUuid,
    ) -> Result<bool, AppError> {
        Ok(is_custodian_of(carbon, &self.custody(silicon).await?))
    }

    /// The cached Silicons an account looks after.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn custodial_silicons(
        &self,
        carbon: &AccountUuid,
    ) -> Result<Vec<AccountRecord>, AppError> {
        self.store
            .custodial_silicons(carbon.as_str())
            .await
            .map_err(database_error)
    }

    /// Resolves a `c:`/`si:` id (or a uuid) a caller named to an active
    /// account.
    ///
    /// # Errors
    ///
    /// Returns 422 `invalid_account_id` for text that is neither, 422
    /// `unknown_account` when no account has that id, and 422
    /// `account_inactive` for a deleted, unclaimed or pending account.
    pub async fn resolve(&self, input: &str) -> Result<AccountRecord, AppError> {
        let record = if is_account_uuid(input) {
            let uuid = AccountUuid::new(input).map_err(|error| {
                AppError::unprocessable("invalid_account_id", error.to_string())
            })?;
            self.record(&uuid).await?
        } else {
            let id = PublicId::parse(input).map_err(|error| {
                AppError::unprocessable("invalid_account_id", error.to_string())
            })?;
            self.resolve_public_id(&id).await?
        };
        if record.status == AccountStatus::Deleted {
            return Err(AppError::unprocessable(
                "account_inactive",
                format!("{input} is a deleted account; it cannot be shared with."),
            ));
        }
        Ok(record)
    }

    /// Public ids move between accounts (an id given up can be taken by
    /// someone else), so naming a recipient asks Silicon Accounts first. The
    /// cache answers only when Silicon Accounts cannot (budget spent or
    /// unreachable) and it holds exactly one fresh account for the id.
    async fn resolve_public_id(&self, id: &PublicId) -> Result<AccountRecord, AppError> {
        let looked_up = match self.gateway.lookup_by_id(id.as_str()).await {
            Ok(found) => found,
            Err(
                error @ (AccountsError::RateLimited { .. } | AccountsError::Unavailable { .. }),
            ) => {
                let cached = self
                    .store
                    .find_by_public_id(id.as_str())
                    .await
                    .map_err(database_error)?;
                if let [record] = cached.as_slice()
                    && self.is_fresh(record)
                {
                    tracing::warn!(
                        event = "accounts_lookup_fell_back_to_cache",
                        error = %error,
                        "resolved an account id from the cache"
                    );
                    return Ok(record.clone());
                }
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        match looked_up {
            Some(summary) => {
                require_active(&summary, id.as_str())?;
                self.store
                    .apply_snapshot(&snapshot_from_summary(&summary))
                    .await
                    .map_err(database_error)
            }
            None => Err(AppError::unprocessable(
                "unknown_account",
                format!(
                    "No Carbon or Silicon has the id {id} at Silicon Accounts. Check the spelling; ids look like c:ada or si:scout."
                ),
            )),
        }
    }

    /// Returns the account's own cached profile, refreshed from Silicon
    /// Accounts with its token when missing or stale.
    ///
    /// # Errors
    ///
    /// Returns the database error, or a lookup error when nothing is cached.
    pub async fn profile(
        &self,
        actor: &Actor,
        access_token: Option<&str>,
    ) -> Result<AccountRecord, AppError> {
        let cached = self
            .store
            .find(actor.uuid.as_str())
            .await
            .map_err(database_error)?;
        let stale = cached
            .as_ref()
            .is_none_or(|record| record.display_name.is_none() || !self.is_fresh(record));
        if stale && let Some(token) = access_token {
            match self.gateway.userinfo(token).await {
                Ok(info) => {
                    let shared_email = actor
                        .scope()
                        .is_some_and(|scope| scope.split_whitespace().any(|item| item == "email"));
                    return self
                        .store
                        .apply_snapshot(&snapshot_from_account(&info.account, shared_email))
                        .await
                        .map_err(database_error);
                }
                Err(error) => tracing::warn!(
                    event = "accounts_userinfo_failed",
                    error = %error,
                    "serving the cached profile"
                ),
            }
        }
        match cached {
            Some(record) => Ok(record),
            None => self.record(&actor.uuid).await,
        }
    }

    /// Applies one verified webhook delivery.
    ///
    /// # Errors
    ///
    /// Returns 400 when a verified body names an invalid account, and the
    /// database error otherwise (Silicon Accounts then retries).
    pub async fn apply_webhook(&self, event: &WebhookEvent) -> Result<WebhookOutcome, AppError> {
        let delivery = delivery_from_event(event)?;
        let outcome = self
            .store
            .apply_webhook(&delivery)
            .await
            .map_err(database_error)?;
        if let (
            Some(uuid),
            AccountChange::SignedOut | AccountChange::AccessRemoved | AccountChange::Deleted,
        ) = (&delivery.account_uuid, &delivery.change)
        {
            self.gateway.forget_introspections_for(uuid);
        }
        Ok(outcome)
    }

    async fn refresh(&self, uuid: &AccountUuid) -> Result<Option<AccountRecord>, AppError> {
        match self.gateway.lookup(uuid.as_str()).await? {
            Some(summary) => self
                .store
                .apply_snapshot(&snapshot_from_summary(&summary))
                .await
                .map(Some)
                .map_err(database_error),
            None => Ok(None),
        }
    }

    fn is_fresh(&self, record: &AccountRecord) -> bool {
        let ttl = time::Duration::try_from(self.ttl).unwrap_or(time::Duration::MAX);
        record.refreshed_at + ttl > OffsetDateTime::now_utc()
    }
}

/// Refuses a token issued before the account's sign-ins ended.
fn refuse_ended(record: &AccountRecord, issued_at: OffsetDateTime) -> Result<(), AppError> {
    if record.status == AccountStatus::Deleted {
        return Err(AppError::unauthorized(
            "account_deleted",
            "This account was deleted at Silicon Accounts; Briefcase no longer serves it.",
        ));
    }
    if record
        .revoked_before
        .is_some_and(|revoked| issued_at <= revoked)
    {
        return Err(AppError::unauthorized(
            "token_revoked",
            "This access token was issued before the account's sign-in at Briefcase ended (it signed out, removed Briefcase's access, or its STK was rotated). Sign in again.",
        ));
    }
    Ok(())
}

fn require_active(summary: &AccountSummary, input: &str) -> Result<(), AppError> {
    match summary.status.as_str() {
        "active" | "" => Ok(()),
        status => Err(AppError::unprocessable(
            "account_inactive",
            format!(
                "{input} is not an active account at Silicon Accounts (its status is {status}); it cannot be shared with until it is active."
            ),
        )),
    }
}

fn delivery_from_event(event: &WebhookEvent) -> Result<WebhookDelivery, AppError> {
    let occurred_at = event.occurred_at.unwrap_or_else(OffsetDateTime::now_utc);
    let (account_uuid, change) = match &event.payload {
        WebhookPayload::AccountIdChanged(data) => (
            Some(data.uuid.clone()),
            AccountChange::IdChanged {
                kind: data.kind.map(actor_kind),
                old_id: data.old_id.clone(),
                new_id: data.new_id.clone(),
            },
        ),
        WebhookPayload::AccountUpdated(data) => (
            Some(data.uuid.clone()),
            data.account
                .as_ref()
                .map_or(AccountChange::Ignore, |account| {
                    let shared_email = account.email.is_some()
                        || data.changed.iter().any(|field| field == "email");
                    AccountChange::Updated(Box::new(snapshot_from_account(account, shared_email)))
                }),
        ),
        WebhookPayload::CustodianChanged(data) => (
            Some(data.uuid.clone()),
            data.to.as_ref().map_or(AccountChange::Ignore, |to| {
                AccountChange::CustodianChanged {
                    custodian_uuid: to.uuid.clone(),
                }
            }),
        ),
        WebhookPayload::MembershipSignedOut(data) => (
            Some(data.uuid.clone()),
            if data.reason.as_deref() == Some("app_revoked") {
                // Briefcase itself revoked one sign-in (a CLI logout or the
                // web's sign-out): the account's other sign-ins stay valid.
                AccountChange::Ignore
            } else {
                AccountChange::SignedOut
            },
        ),
        WebhookPayload::MembershipAccessRemoved(data) => {
            (Some(data.uuid.clone()), AccountChange::AccessRemoved)
        }
        WebhookPayload::AccountDeleted(data) => (Some(data.uuid.clone()), AccountChange::Deleted),
        _ => (None, AccountChange::Ignore),
    };
    if let Some(uuid) = &account_uuid
        && !is_account_uuid(uuid)
    {
        return Err(AppError::bad_request("webhook_invalid_account"));
    }
    if let AccountChange::CustodianChanged { custodian_uuid } = &change
        && !is_account_uuid(custodian_uuid)
    {
        return Err(AppError::bad_request("webhook_invalid_account"));
    }
    Ok(WebhookDelivery {
        event_id: event.event_id.clone(),
        event_type: event.event_type.clone(),
        occurred_at,
        account_uuid,
        change,
    })
}

fn snapshot_from_summary(summary: &AccountSummary) -> AccountSnapshot {
    AccountSnapshot {
        uuid: summary.uuid.clone(),
        kind: actor_kind(summary.kind),
        public_id: non_empty(&summary.id),
        display_name: non_empty(&summary.display_name),
        pfp_url: non_empty(&summary.pfp_url),
        email: None,
        custodian_uuid: summary
            .custodian
            .as_ref()
            .map(|custodian| custodian.uuid.clone()),
        remote_status: summary.status.clone(),
        version: None,
    }
}

fn snapshot_from_account(account: &AccountForApp, shared_email: bool) -> AccountSnapshot {
    AccountSnapshot {
        uuid: account.uuid.clone(),
        kind: actor_kind(account.kind),
        public_id: non_empty(&account.id),
        display_name: non_empty(&account.display_name),
        pfp_url: non_empty(&account.pfp_url),
        email: shared_email.then(|| account.email.clone()),
        custodian_uuid: account
            .custodian
            .as_ref()
            .map(|custodian| custodian.uuid.clone()),
        remote_status: "active".to_owned(),
        version: (account.version > 0).then_some(account.version),
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

const fn actor_kind(kind: AccountKind) -> ActorKind {
    match kind {
        AccountKind::Carbon => ActorKind::Carbon,
        AccountKind::Silicon => ActorKind::Silicon,
    }
}

fn display_id(record: &AccountRecord) -> &str {
    record.public_id.as_deref().unwrap_or(&record.uuid)
}

fn invalid_proof() -> AppError {
    AppError::unauthorized(
        "proof_invalid",
        "The proof is not valid for Briefcase right now: it is unknown, expired or revoked, it was issued for another app, or the account's sign-in at the issuing app ended. Ask the issuing app for a new proof.",
    )
}

fn unknown_account(uuid: &str) -> AppError {
    AppError::unprocessable(
        "unknown_account",
        format!("No Carbon or Silicon has the uuid {uuid} at Silicon Accounts."),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "passed straight to Result::map_err"
)]
fn database_error(error: sqlx::Error) -> AppError {
    tracing::error!(event = "account_cache_failed", error = %error, "account cache query failed");
    AppError::Internal {
        category: "account_cache",
    }
}
