//! Cloneable dependencies shared by HTTP handlers.

use std::{path::PathBuf, sync::Arc};

use crate::{
    application::{
        content::{ContentRepository, ContentService},
        delegated_upload::{DelegatedUploadRepository, DelegatedUploadService},
        ports::ObjectStore,
        service::MetadataService,
    },
    config::WebhookSettings,
    infrastructure::accounts::AccountDirectory,
};
use sqlx::PgPool;

use super::mapping::ResponseMapper;

pub(crate) type ContentUseCases = ContentService<dyn ContentRepository, dyn ObjectStore>;
pub(crate) type DelegatedUploadUseCases =
    DelegatedUploadService<dyn DelegatedUploadRepository, dyn ObjectStore>;

/// Immutable process dependencies cloned into Axum route services.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) accounts: Arc<AccountDirectory>,
    pub(crate) metadata: MetadataService,
    pub(crate) content: Arc<ContentUseCases>,
    pub(crate) content_adapter: Arc<crate::infrastructure::postgres::PostgresContentRepository>,
    pub(crate) objects: Arc<dyn ObjectStore>,
    pub(crate) delegated_uploads: Arc<DelegatedUploadUseCases>,
    pub(crate) database: PgPool,
    pub(crate) contract_database: Option<PgPool>,
    pub(crate) mapper: ResponseMapper,
    pub(crate) temporary_directory: PathBuf,
    pub(crate) webhook_settings: WebhookSettings,
}

impl AppState {
    /// The response mapper naming each listed drive (by key, its owner's
    /// uuid) by its owner's current `c:`/`si:` id in permanent addresses.
    pub(crate) async fn mapper_for<I, S>(
        &self,
        drives: I,
    ) -> Result<ResponseMapper, crate::error::AppError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut keys: Vec<String> = drives.into_iter().map(Into::into).collect();
        keys.sort_unstable();
        keys.dedup();
        let labels = self.accounts.labels(&keys).await?;
        Ok(self.mapper.with_labels(labels))
    }
}

impl AppState {
    /// Renders one entry with its drive named by its owner's current id.
    pub(crate) async fn entry_dto(
        &self,
        view: crate::application::service::AuthorizedEntryView,
    ) -> Result<super::dto::EntryDto, crate::error::AppError> {
        let drive = view.entry.organization_id.as_str().to_owned();
        self.mapper_for([drive]).await?.entry(view)
    }
}
