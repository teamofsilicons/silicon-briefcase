//! Which drive a request acts in, and the caller's rights there.
//!
//! Authentication puts every request in the caller's own drive. Before a
//! handler calls a use case it moves the request into the drive the use case
//! acts on: the drive holding the addressed entry, or a drive the caller
//! named (`?drive=c:ada`). The caller's relation to that drive's owner decides
//! its rights: its own drive and the drives of the Silicons it looks after
//! carry the owner's rights; anywhere else only shares and creator rights
//! apply. Row security then confines the whole request to that one drive.

use crate::{
    application::context::ExecutionContext,
    domain::{
        account::{AccountUuid, Actor, is_custodian_of},
        actor::{ActorId, ActorKind, ActorRef, DriveAccess},
        ids::EntryId,
    },
    error::AppError,
};

use super::state::AppState;

/// Moves a request about one entry into the drive that holds it.
///
/// An entry Briefcase does not know leaves the request in the caller's own
/// drive, where the lookup answers 404 exactly like an entry the caller may
/// not see.
///
/// # Errors
///
/// Returns 404 for a confined app's (proof) request about another account's
/// drive, and database or Silicon Accounts errors.
pub(crate) async fn for_entry(
    state: &AppState,
    context: ExecutionContext,
    entry_id: EntryId,
) -> Result<ExecutionContext, AppError> {
    let drive = state
        .content_adapter
        .metadata_repository()
        .entry_drive(entry_id.as_uuid())
        .await
        .map_err(database_error)?;
    match drive {
        Some(drive) => enter(state, context, &drive).await,
        None => own(state, context).await,
    }
}

/// Moves a request into the drive the caller named by `c:`/`si:` id or uuid,
/// or keeps it in the caller's own drive when it named none.
///
/// # Errors
///
/// Returns 422 for an id that names no account, and the errors of
/// [`enter`].
pub(crate) async fn named(
    state: &AppState,
    context: ExecutionContext,
    selector: Option<&str>,
) -> Result<ExecutionContext, AppError> {
    let Some(selector) = selector.map(str::trim).filter(|value| !value.is_empty()) else {
        return own(state, context).await;
    };
    let record = state.accounts.resolve(selector).await?;
    enter(state, context, &record.uuid).await
}

/// Keeps a request in the caller's own drive, with the creator rights of the
/// Silicons it looks after.
///
/// # Errors
///
/// Returns database errors.
pub(crate) async fn own(
    state: &AppState,
    context: ExecutionContext,
) -> Result<ExecutionContext, AppError> {
    let Some(uuid) = context
        .authorization()
        .account()
        .map(|account| account.uuid.as_str().to_owned())
    else {
        return Ok(context);
    };
    enter(state, context, &uuid).await
}

/// Moves a request into the drive whose key (its owner's uuid) is `drive`.
///
/// # Errors
///
/// Returns 404 for a confined app's (proof) request about another account's
/// drive or a drive whose owner Silicon Accounts does not know, and database
/// or Silicon Accounts errors.
pub(crate) async fn enter(
    state: &AppState,
    context: ExecutionContext,
    drive: &str,
) -> Result<ExecutionContext, AppError> {
    let authorization = context.authorization();
    // Contexts built without a Silicon Accounts credential (internal callers
    // and tests) already name their drive.
    let Some(account) = authorization.account().cloned() else {
        return Ok(context);
    };
    let custodial = custodial_silicons(state, &account).await?;
    let caller = authorization.actor().clone();
    if drive == account.uuid.as_str() {
        let moved = authorization
            .in_drive(caller, DriveAccess::Owner)
            .map_err(|_| internal())?
            .with_custodial_silicons(custodial);
        return Ok(context.with_authorization(moved));
    }
    if account.confined_app().is_some() {
        // An app acting for an account reaches only that account's own drive
        // (and there only its own folder). First-party apps the operator
        // lists reach everything the account can.
        return Err(AppError::NotFound);
    }
    let drive_uuid = AccountUuid::new(drive).map_err(|_| AppError::NotFound)?;
    let facts = match state.accounts.custody(&drive_uuid).await {
        Ok(facts) => facts,
        Err(AppError::Explained { code, .. }) if code == "unknown_account" => {
            return Err(AppError::NotFound);
        }
        Err(error) => return Err(error),
    };
    let access = if account.kind == ActorKind::Carbon && is_custodian_of(&account.uuid, &facts) {
        DriveAccess::Custodian
    } else {
        DriveAccess::Visitor
    };
    let owner = ActorRef::new(
        facts.kind,
        ActorId::new(drive_uuid.as_str()).map_err(|_| internal())?,
    );
    let moved = authorization
        .in_drive(owner, access)
        .map_err(|_| internal())?
        .with_custodial_silicons(custodial);
    Ok(context.with_authorization(moved))
}

/// The Silicons a Carbon looks after: what they create anywhere carries
/// creator rights for their custodian. An app confined to its own folder
/// never carries the account's custodial powers; a first-party app acts with
/// everything the account can do.
async fn custodial_silicons(state: &AppState, account: &Actor) -> Result<Vec<ActorId>, AppError> {
    if account.kind != ActorKind::Carbon || account.confined_app().is_some() {
        return Ok(Vec::new());
    }
    Ok(state
        .accounts
        .custodial_silicons(&account.uuid)
        .await?
        .into_iter()
        .filter_map(|record| ActorId::new(record.uuid).ok())
        .collect())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "passed straight to Result::map_err"
)]
fn database_error(error: sqlx::Error) -> AppError {
    tracing::error!(event = "drive_lookup_failed", error = %error, "drive lookup failed");
    AppError::Internal {
        category: "drive_lookup",
    }
}

const fn internal() -> AppError {
    AppError::Internal {
        category: "drive_context",
    }
}
