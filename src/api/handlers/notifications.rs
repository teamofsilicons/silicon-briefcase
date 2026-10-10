//! Central notification inbox handlers.

use axum::{Json, extract::State, http::HeaderMap};

use crate::error::AppError;

use super::super::{
    auth::Action, dto::NotificationInboxDto, extract, mapping::metadata_error, state::AppState,
};

/// Returns the caller's twenty newest notifications and unread badge count.
pub(crate) async fn list_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<NotificationInboxDto>, AppError> {
    let context =
        extract::authenticate(&state, &headers, Action::ListNotifications, "notifications").await?;
    let organization = extract::tenant(&context);
    let inbox = extract::scoped(&context, state.metadata.notification_inbox(&context))
        .await
        .map_err(metadata_error)?;
    let drives = std::iter::once(organization.clone()).chain(
        inbox
            .items
            .iter()
            .filter_map(|item| item.subject.as_ref()?.drive.clone()),
    );
    Ok(Json(
        state
            .mapper_for(drives)
            .await?
            .inbox(&organization, inbox)?,
    ))
}

/// Marks the caller's entire inbox read and returns it afterwards.
pub(crate) async fn read_notifications(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<NotificationInboxDto>, AppError> {
    let context =
        extract::authenticate(&state, &headers, Action::ReadNotifications, "notifications").await?;
    let organization = extract::tenant(&context);
    let metadata = extract::mutation(
        &headers,
        "mark_notifications_read",
        &organization,
        &(),
        false,
    )?;
    let inbox = extract::scoped(
        &context,
        state.metadata.mark_notifications_read(&context, &metadata),
    )
    .await
    .map_err(metadata_error)?;
    let drives = std::iter::once(organization.clone()).chain(
        inbox
            .items
            .iter()
            .filter_map(|item| item.subject.as_ref()?.drive.clone()),
    );
    Ok(Json(
        state
            .mapper_for(drives)
            .await?
            .inbox(&organization, inbox)?,
    ))
}
