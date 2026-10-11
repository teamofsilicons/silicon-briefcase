//! Silicon Briefcase durable background worker process.

use silicon_briefcase::{config::WorkerProcessSettings, logging, worker};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = WorkerProcessSettings::from_env()?;
    logging::init_process(settings.environment, &settings.log_filter)?;
    worker::run(settings).await
}
