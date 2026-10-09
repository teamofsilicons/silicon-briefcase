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
