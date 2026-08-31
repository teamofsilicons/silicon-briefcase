//! Silicon Briefcase durable background worker process.

use silicon_briefcase::{config::WorkerProcessSettings, telemetry, worker};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = WorkerProcessSettings::from_env()?;
    telemetry::init_process(settings.environment, &settings.log_filter)?;
    worker::run(settings).await
}
