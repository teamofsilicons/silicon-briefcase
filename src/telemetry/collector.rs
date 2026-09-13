//! Space Station adapter for operational events, with a fixed privacy boundary.
use briefcase_client::telemetry::{Event, Source, Stage};
use std::{
    path::PathBuf,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{
    Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

static COLLECTOR: OnceLock<Option<space_station::SpaceClient>> = OnceLock::new();

tokio::task_local! { pub(super) static REQUEST: RequestContext; }

#[derive(Clone, Copy)]
pub(crate) struct RequestContext {
    pub enabled: bool,
    pub testing: bool,
    pub request_id: Option<uuid::Uuid>,
    pub environment_id: Option<uuid::Uuid>,
}

fn options() -> Option<(String, String, PathBuf)> {
    if !briefcase_client::telemetry::enabled_from_env() {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let private = home
        .as_ref()
        .map(|home| home.join(".config/briefcase/telemetry.env"));
    let file = private
        .and_then(|file| std::fs::read_to_string(file).ok())
        .unwrap_or_default();
    let setting = |name: &str| {
        std::env::var(name).ok().or_else(|| {
            file.lines()
                .find_map(|line| line.strip_prefix(&format!("{name}=")).map(str::to_owned))
        })
    };
    let key = setting("BRIEFCASE_TELEMETRY_TABLE_KEY")?;
    if !key.starts_with("table-siliconbriefcase-") {
        return None;
    }
    let url = setting("BRIEFCASE_TELEMETRY_URL")
        .unwrap_or_else(|| "https://backend.spacestation.teamofsilicons.com".into());
    let parsed = url::Url::parse(&url).ok()?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !(parsed.scheme() == "https"
            || parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        return None;
    }
    let spool = std::env::var_os("BRIEFCASE_TELEMETRY_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".briefcase-telemetry")))?;
    Some((key, url, spool))
}

pub(super) fn init() {
    // Reqwest and Space Station enable different rustls providers. Select one
    // before Space Station opens its WebSocket on a background thread.
    let _ = rustls::crypto::ring::default_provider().install_default();
    COLLECTOR.get_or_init(|| {
        let (key, url, home) = options()?;
        space_station::SpaceClient::builder(&key)
            .url(url)
            .home(home)
            .flush_timeout(Duration::from_millis(100))
            .on_error(|_| {})
            .build()
            .ok()
    });
}

/// Whether the process has a configured collector; never exposes its key.
pub fn configured() -> bool {
    COLLECTOR.get().is_some_and(Option::is_some)
}

/// Queues a bounded event through Space Station's durable spool.
pub fn record(event: Event, client_reported: bool) {
    if !event.valid()
        || !client_reported
            && REQUEST
                .try_with(|context| !context.enabled)
                .unwrap_or(false)
    {
        return;
    }
    let Some(Some(client)) = COLLECTOR.get() else {
        return;
    };
    let Ok(mut value) = serde_json::to_value(event) else {
        return;
    };
    value["schema_version"] = 1.into();
    value["service"] = "briefcase".into();
    value["build"] = env!("CARGO_PKG_VERSION").into();
    value["client_reported"] = client_reported.into();
    value["received_at_ms"] = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
    .into();
    client.record(value);
}

/// Gives the spool a short shutdown window; diagnostics never block indefinitely.
pub fn flush() -> bool {
    COLLECTOR
        .get()
        .and_then(Option::as_ref)
        .is_none_or(space_station::SpaceClient::flush)
}

pub(super) struct CollectorLayer;
impl<S: Subscriber> Layer<S> for CollectorLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if !configured()
            || !event.metadata().target().starts_with("silicon_briefcase::")
            || event.metadata().target().ends_with("::api::middleware")
        {
            return;
        }
        if let Some(value) = observation(event) {
            record(value, false);
        }
    }
}

fn observation(event: &tracing::Event<'_>) -> Option<Event> {
    if REQUEST
        .try_with(|context| !context.enabled)
        .unwrap_or(false)
    {
        return None;
    }
    let mut fields = SafeFields::default();
    event.record(&mut fields);
    let metadata = event.metadata();
    let source = if metadata.target().contains("::worker") {
        Source::Worker
    } else {
        Source::Backend
    };
    let stage = if *metadata.level() <= tracing::Level::WARN {
        Stage::Failed
    } else {
        Stage::Progress
    };
    let mut value = Event::new(
        source,
        fields
            .operation
            .unwrap_or_else(|| metadata.target().replace("::", ".")),
        stage,
    );
    value.testing = REQUEST.try_with(|context| context.testing).unwrap_or(false)
        || value.operation.starts_with("test_")
        || value.operation.starts_with("testing_environment_");
    value.request_id = REQUEST
        .try_with(|context| context.request_id)
        .ok()
        .flatten();
    value.environment_id = REQUEST
        .try_with(|context| context.environment_id)
        .ok()
        .flatten();
    if value.operation.ends_with("_completed") || value.operation.ends_with("_shutdown") {
        value.stage = Stage::Completed;
    }
    if value.operation.ends_with("_started") {
        value.stage = Stage::Started;
    }
    value.count = fields.count;
    value.attempt = fields.attempt;
    Some(value)
}

#[derive(Default)]
struct SafeFields {
    operation: Option<String>,
    count: Option<u64>,
    attempt: Option<u32>,
}
impl Visit for SafeFields {
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "event" && Event::new(Source::Backend, value, Stage::Progress).valid() {
            self.operation = Some(value.into());
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "count" | "processed" | "claimed" | "retired" | "purged" | "rows" => {
                self.count = Some(value);
            }
            "attempt" => self.attempt = u32::try_from(value).ok(),
            _ => (),
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if let Ok(value) = u64::try_from(value) {
            self.record_u64(field, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt as _;

    struct Capture(Arc<Mutex<Vec<Event>>>);
    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if let Some(value) = observation(event) {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(value);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tracing_keeps_correlation_and_progress_but_drops_content_and_opted_out_work()
    -> anyhow::Result<()> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(events.clone()));
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let request_id = uuid::Uuid::new_v4();
        let environment_id = uuid::Uuid::new_v4();
        let context = RequestContext {
            enabled: true,
            testing: true,
            request_id: Some(request_id),
            environment_id: Some(environment_id),
        };
        REQUEST.scope(context, async {
            tracing::info!(target: "silicon_briefcase::worker", event = "upload_completed", count = 42u64, attempt = 2u64, token = "slt_private", path = "private/secret.txt", "private message");
        }).await;
        REQUEST
            .scope(
                RequestContext {
                    enabled: false,
                    ..context
                },
                async {
                    tokio::task::yield_now().await;
                    tracing::error!(event = "hidden_failure", "private");
                },
            )
            .await;
        let events = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source, Source::Worker);
        assert_eq!(events[0].operation, "upload_completed");
        assert_eq!(events[0].request_id, Some(request_id));
        assert_eq!(events[0].environment_id, Some(environment_id));
        assert_eq!(events[0].count, Some(42));
        assert_eq!(events[0].attempt, Some(2));
        let stored = serde_json::to_string(&events[0])?;
        assert!(
            !stored.contains("private") && !stored.contains("secret") && !stored.contains("token")
        );
        Ok(())
    }
}
