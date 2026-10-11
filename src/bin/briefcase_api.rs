//! Silicon Briefcase HTTP API process.

use silicon_briefcase::{api, config::Settings, logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    logging::init(&settings)?;
    api::serve(settings).await
}
