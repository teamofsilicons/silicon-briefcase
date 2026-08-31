//! Silicon Briefcase HTTP API process.

use silicon_briefcase::{api, config::Settings, telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    telemetry::init(&settings)?;
    api::serve(settings).await
}
