//! Cloneable dependencies shared by HTTP handlers.

use std::{path::PathBuf, sync::Arc};

use crate::{
    application::{
        content::{ContentRepository, ContentService},
        ports::ObjectStore,
        service::MetadataService,
        webhook::IamWebhookRepository,
    },
    config::WebhookSettings,
    infrastructure::iam::IamClient,
    infrastructure::testing::TestingEnvironmentStore,
};
use sqlx::PgPool;

use super::mapping::ResponseMapper;

pub(crate) type ContentUseCases = ContentService<dyn ContentRepository, dyn ObjectStore>;

/// Immutable process dependencies cloned into Axum route services.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) iam: Arc<IamClient>,
    pub(crate) metadata: MetadataService,
    pub(crate) content: Arc<ContentUseCases>,
    pub(crate) webhook_repository: Arc<dyn IamWebhookRepository>,
    pub(crate) database: PgPool,
    pub(crate) mapper: ResponseMapper,
    pub(crate) temporary_directory: PathBuf,
    pub(crate) webhook_settings: WebhookSettings,
    pub(crate) testing: Option<Arc<TestingEnvironmentStore>>,
}
