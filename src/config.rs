//! Typed, validated runtime configuration loaded from environment variables.

use std::{
    env,
    net::{IpAddr, SocketAddr},
    num::{NonZeroU32, NonZeroUsize},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use secrecy::{ExposeSecret as _, SecretString};
use thiserror::Error;
use url::Url;

const MAX_IAM_RESPONSE_BYTES: usize = 1_048_576;
const MIN_SECRET_BYTES: usize = 32;

/// Fully validated API process configuration.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Deployment environment and its transport-safety policy.
    pub environment: RuntimeEnvironment,
    /// HTTP listener and request limits.
    pub server: ServerSettings,
    /// PostgreSQL runtime pool settings.
    pub database: DatabaseSettings,
    /// Silicon IAM service integration.
    pub iam: IamSettings,
    /// Platform-owned S3 storage defaults.
    pub s3: S3Settings,
    /// Inbound IAM webhook verification settings.
    pub webhook: WebhookSettings,
    /// Tracing filter directive.
    pub log_filter: String,
}

/// Minimal configuration accepted by the privileged cross-tenant worker.
#[derive(Clone, Debug)]
pub struct WorkerProcessSettings {
    /// Deployment environment and its transport-safety policy.
    pub environment: RuntimeEnvironment,
    /// Privileged PostgreSQL connection used only by the worker.
    pub database: DatabaseSettings,
    /// S3 client transport and credential-chain settings used for cleanup.
    pub s3: S3Settings,
    /// Durable background worker policy.
    pub worker: WorkerSettings,
    /// Tracing filter directive.
    pub log_filter: String,
}

/// Minimal configuration accepted by the privileged migration process.
#[derive(Clone, Debug)]
pub struct MigrationSettings {
    /// Deployment environment and its transport-safety policy.
    pub environment: RuntimeEnvironment,
    /// Privileged PostgreSQL connection used only for migrations.
    pub database: DatabaseSettings,
    /// Tracing filter directive.
    pub log_filter: String,
}

/// Deployment environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeEnvironment {
    /// Local development process.
    Development,
    /// Automated test process.
    Test,
    /// Production process.
    Production,
}

/// HTTP listener, public URL, and request-budget settings.
#[derive(Clone, Debug)]
pub struct ServerSettings {
    /// Address on which the API listens.
    pub bind_addr: SocketAddr,
    /// Canonical externally visible API base URL.
    pub public_base_url: Url,
    /// Canonical application base URL that serves clean permanent entry URLs.
    pub public_site_base_url: Url,
    /// Deadline for ordinary JSON requests.
    pub request_timeout: Duration,
    /// Deadline for streaming upload requests.
    pub upload_timeout: Duration,
    /// Deadline for one synchronous historical-version restore.
    pub restore_timeout: Duration,
    /// Maximum JSON request size.
    pub max_json_body_bytes: NonZeroUsize,
    /// Maximum concurrently admitted requests per API process.
    pub max_concurrent_requests: NonZeroUsize,
    /// Maximum concurrently admitted restore requests per API process.
    pub max_concurrent_restores: NonZeroUsize,
    /// Deadline for graceful process shutdown.
    pub shutdown_timeout: Duration,
}

/// PostgreSQL pool and query-budget settings.
#[derive(Clone, Debug)]
pub struct DatabaseSettings {
    /// PostgreSQL connection URL.
    pub url: SecretString,
    /// Maximum open connections per process.
    pub max_connections: NonZeroU32,
    /// Minimum idle connections per process.
    pub min_connections: u32,
    /// Pool acquisition deadline.
    pub acquire_timeout: Duration,
    /// Per-statement database deadline.
    pub statement_timeout: Duration,
}

/// Silicon IAM endpoints, application identity, and HTTP budgets.
#[derive(Clone, Debug)]
pub struct IamSettings {
    /// IAM API base URL.
    pub base_url: Url,
    /// Online opaque-token introspection endpoint.
    pub bearer_introspection_url: Url,
    /// Current actor and organization claims endpoint.
    pub bearer_userinfo_url: Url,
    /// Online OBO proof verification and consumption endpoint.
    pub obo_verification_url: Url,
    /// Briefcase IAM application identifier.
    pub app_id: String,
    /// Briefcase IAM application secret.
    pub app_secret: SecretString,
    /// Audience required on every accepted OBO proof.
    pub audience: String,
    /// TCP/TLS connection establishment deadline.
    pub connect_timeout: Duration,
    /// Complete IAM request deadline.
    pub request_timeout: Duration,
    /// Hard upper bound for one IAM response body.
    pub max_response_bytes: NonZeroUsize,
}

/// Default platform S3 configuration.
#[derive(Clone, Debug)]
pub struct S3Settings {
    /// AWS region containing the platform bucket.
    pub region: String,
    /// Platform-owned bucket name.
    pub bucket: String,
    /// Key prefix below which organization-scoped opaque keys are placed.
    pub key_prefix: String,
    /// Optional S3-compatible endpoint, primarily for local tests.
    pub endpoint_url: Option<Url>,
    /// Whether the SDK must use path-style bucket addressing.
    pub force_path_style: bool,
    /// Server-side encryption applied to newly stored objects.
    pub encryption: S3Encryption,
    /// Private directory used for bounded streaming temporary files.
    pub temporary_directory: PathBuf,
    /// Deadline for one S3 control- or data-plane operation.
    pub operation_timeout: Duration,
}

/// Server-side encryption for the platform bucket.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum S3Encryption {
    /// S3-managed encryption keys.
    SseS3,
    /// A configured AWS KMS key.
    SseKms {
        /// Full ARN of the KMS key.
        key_arn: String,
    },
}

/// IAM webhook authentication and replay-window settings.
#[derive(Clone, Debug)]
pub struct WebhookSettings {
    /// HMAC key shared with IAM.
    pub signing_secret: SecretString,
    /// IAM webhook signing-key version accepted by this deployment.
    pub signing_key_version: NonZeroU32,
    /// Maximum accepted difference between signature and server time.
    pub replay_window: Duration,
    /// Maximum webhook request body size.
    pub max_body_bytes: NonZeroUsize,
}

/// Durable worker polling, leasing, and retry policy.
#[derive(Clone, Debug)]
pub struct WorkerSettings {
    /// Maximum jobs claimed in one batch.
    pub batch_size: NonZeroUsize,
    /// Maximum concurrent provider cleanup calls per worker process.
    pub cleanup_concurrency: NonZeroUsize,
    /// Idle polling interval.
    pub poll_interval: Duration,
    /// Duration of a claimed job lease.
    pub lease_duration: Duration,
    /// Maximum delivery attempts before terminal failure.
    pub max_attempts: u16,
    /// Maximum retry delay.
    pub max_retry_delay: Duration,
    /// Interval between retention and reconciliation passes.
    pub maintenance_interval: Duration,
}

/// Configuration loading or validation failure.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// A required environment variable is absent or blank.
    #[error("required environment variable {0} is missing")]
    Missing(&'static str),
    /// A variable cannot be parsed or violates a safety invariant.
    #[error("invalid environment variable {name}: {reason}")]
    Invalid {
        /// Environment variable name.
        name: &'static str,
        /// Redacted reason that never contains the supplied value.
        reason: String,
    },
}

impl Settings {
    /// Loads and validates API settings from the process environment.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when a required value is absent, malformed, or
    /// unsafe for the selected deployment environment.
    pub fn from_env() -> Result<Self, SettingsError> {
        let environment = parse_or("BRIEFCASE_ENVIRONMENT", "development")?;
        let server = server_settings()?;
        let database = database_settings("BRIEFCASE_DATABASE_URL", false)?;
        let iam = iam_settings()?;
        let s3 = s3_settings()?;
        let webhook = webhook_settings()?;
        validate_environment_safety(environment, &server, &database, &iam, &s3)?;

        Ok(Self {
            environment,
            server,
            database,
            iam,
            s3,
            webhook,
            log_filter: value_or(
                "BRIEFCASE_LOG_FILTER",
                "silicon_briefcase=info,tower_http=info",
            ),
        })
    }
}

impl WorkerProcessSettings {
    /// Loads the isolated worker settings and privileged database connection.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when worker policy or database transport
    /// settings are absent, malformed, or unsafe.
    pub fn from_env() -> Result<Self, SettingsError> {
        let environment = parse_or("BRIEFCASE_ENVIRONMENT", "development")?;
        let database = database_settings("BRIEFCASE_WORKER_DATABASE_URL", false)?;
        let s3 = s3_settings()?;
        let worker = worker_settings()?;
        validate_database_transport(environment, &database, "BRIEFCASE_WORKER_DATABASE_URL")?;
        validate_storage_environment_safety(environment, &s3)?;

        Ok(Self {
            environment,
            database,
            s3,
            worker,
            log_filter: value_or("BRIEFCASE_LOG_FILTER", "silicon_briefcase=info"),
        })
    }
}

impl MigrationSettings {
    /// Loads the isolated migration-process settings.
    ///
    /// # Errors
    ///
    /// Returns a redacted error when the migrator connection or pool policy is
    /// absent, malformed, or unsafe in production.
    pub fn from_env() -> Result<Self, SettingsError> {
        let environment = parse_or("BRIEFCASE_ENVIRONMENT", "development")?;
        let database = database_settings("BRIEFCASE_MIGRATOR_DATABASE_URL", true)?;
        validate_database_transport(environment, &database, "BRIEFCASE_MIGRATOR_DATABASE_URL")?;

        Ok(Self {
            environment,
            database,
            log_filter: value_or("BRIEFCASE_LOG_FILTER", "silicon_briefcase=info"),
        })
    }
}

impl FromStr for RuntimeEnvironment {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "production" | "prod" => Ok(Self::Production),
            _ => Err("must be development, test, or production".to_owned()),
        }
    }
}

fn server_settings() -> Result<ServerSettings, SettingsError> {
    let settings = ServerSettings {
        bind_addr: parse_or("BRIEFCASE_BIND_ADDR", "127.0.0.1:8080")?,
        public_base_url: parse_or(
            "BRIEFCASE_PUBLIC_BASE_URL",
            "https://backend.briefcase.teamofsilicons.com/api/v1/",
        )?,
        public_site_base_url: parse_or(
            "BRIEFCASE_PUBLIC_SITE_BASE_URL",
            "https://briefcase.teamofsilicons.com/",
        )?,
        request_timeout: duration_secs("BRIEFCASE_REQUEST_TIMEOUT_SECONDS", 15)?,
        upload_timeout: duration_secs("BRIEFCASE_UPLOAD_TIMEOUT_SECONDS", 900)?,
        restore_timeout: duration_secs("BRIEFCASE_RESTORE_TIMEOUT_SECONDS", 172_800)?,
        max_json_body_bytes: parse_or("BRIEFCASE_MAX_JSON_BODY_BYTES", "1048576")?,
        max_concurrent_requests: parse_or("BRIEFCASE_MAX_CONCURRENT_REQUESTS", "1024")?,
        max_concurrent_restores: parse_or("BRIEFCASE_MAX_CONCURRENT_RESTORES", "2")?,
        shutdown_timeout: duration_secs("BRIEFCASE_SHUTDOWN_TIMEOUT_SECONDS", 30)?,
    };
    require_nonzero_duration(
        "BRIEFCASE_REQUEST_TIMEOUT_SECONDS",
        settings.request_timeout,
    )?;
    require_nonzero_duration("BRIEFCASE_UPLOAD_TIMEOUT_SECONDS", settings.upload_timeout)?;
    require_nonzero_duration(
        "BRIEFCASE_RESTORE_TIMEOUT_SECONDS",
        settings.restore_timeout,
    )?;
    require_nonzero_duration(
        "BRIEFCASE_SHUTDOWN_TIMEOUT_SECONDS",
        settings.shutdown_timeout,
    )?;
    if settings.max_concurrent_restores > settings.max_concurrent_requests {
        return Err(invalid(
            "BRIEFCASE_MAX_CONCURRENT_RESTORES",
            "must not exceed BRIEFCASE_MAX_CONCURRENT_REQUESTS",
        ));
    }
    validate_service_url(
        "BRIEFCASE_PUBLIC_BASE_URL",
        &settings.public_base_url,
        false,
    )?;
    validate_service_url(
        "BRIEFCASE_PUBLIC_SITE_BASE_URL",
        &settings.public_site_base_url,
        false,
    )?;
    Ok(settings)
}

fn database_settings(
    url_name: &'static str,
    migration: bool,
) -> Result<DatabaseSettings, SettingsError> {
    let (
        max_name,
        max_default,
        min_connections,
        acquire_name,
        acquire_default,
        statement_name,
        statement_default,
    ) = if migration {
        (
            "BRIEFCASE_MIGRATOR_DATABASE_MAX_CONNECTIONS",
            "2",
            0,
            "BRIEFCASE_MIGRATOR_DATABASE_ACQUIRE_TIMEOUT_SECONDS",
            10,
            "BRIEFCASE_MIGRATOR_DATABASE_STATEMENT_TIMEOUT_SECONDS",
            120,
        )
    } else {
        (
            "BRIEFCASE_DATABASE_MAX_CONNECTIONS",
            "24",
            parse_or("BRIEFCASE_DATABASE_MIN_CONNECTIONS", "1")?,
            "BRIEFCASE_DATABASE_ACQUIRE_TIMEOUT_SECONDS",
            3,
            "BRIEFCASE_DATABASE_STATEMENT_TIMEOUT_SECONDS",
            10,
        )
    };
    let settings = DatabaseSettings {
        url: required_secret(url_name)?,
        max_connections: parse_or(max_name, max_default)?,
        min_connections,
        acquire_timeout: duration_secs(acquire_name, acquire_default)?,
        statement_timeout: duration_secs(statement_name, statement_default)?,
    };
    if settings.min_connections > settings.max_connections.get() {
        return Err(invalid(
            if migration {
                "BRIEFCASE_MIGRATOR_DATABASE_MAX_CONNECTIONS"
            } else {
                "BRIEFCASE_DATABASE_MIN_CONNECTIONS"
            },
            "minimum connections cannot exceed maximum connections",
        ));
    }
    require_nonzero_duration(acquire_name, settings.acquire_timeout)?;
    require_nonzero_duration(statement_name, settings.statement_timeout)?;
    validate_database_url(&settings, url_name)?;
    Ok(settings)
}

fn iam_settings() -> Result<IamSettings, SettingsError> {
    let base_url = normalized_base_url(
        "BRIEFCASE_IAM_BASE_URL",
        parse_or(
            "BRIEFCASE_IAM_BASE_URL",
            "https://backend.iam.teamofsilicons.com/api/v1/",
        )?,
    )?;
    let bearer_path = value_or(
        "BRIEFCASE_IAM_BEARER_INTROSPECTION_PATH",
        "auth/tokens/introspect",
    );
    let userinfo_path = value_or("BRIEFCASE_IAM_USERINFO_PATH", "oauth/userinfo");
    let obo_path = value_or("BRIEFCASE_IAM_OBO_VERIFICATION_PATH", "obo-access/verify");
    let settings = IamSettings {
        bearer_introspection_url: join_endpoint(
            "BRIEFCASE_IAM_BEARER_INTROSPECTION_PATH",
            &base_url,
            &bearer_path,
        )?,
        bearer_userinfo_url: join_endpoint(
            "BRIEFCASE_IAM_USERINFO_PATH",
            &base_url,
            &userinfo_path,
        )?,
        obo_verification_url: join_endpoint(
            "BRIEFCASE_IAM_OBO_VERIFICATION_PATH",
            &base_url,
            &obo_path,
        )?,
        base_url,
        app_id: nonempty_value(
            "BRIEFCASE_IAM_APP_ID",
            &value_or("BRIEFCASE_IAM_APP_ID", "silicon-briefcase"),
        )?,
        app_secret: required_secret("BRIEFCASE_IAM_APP_SECRET")?,
        audience: nonempty_value(
            "BRIEFCASE_IAM_AUDIENCE",
            &value_or("BRIEFCASE_IAM_AUDIENCE", "silicon-briefcase"),
        )?,
        connect_timeout: duration_millis("BRIEFCASE_IAM_CONNECT_TIMEOUT_MS", 1_500)?,
        request_timeout: duration_millis("BRIEFCASE_IAM_REQUEST_TIMEOUT_MS", 4_000)?,
        max_response_bytes: parse_or("BRIEFCASE_IAM_MAX_RESPONSE_BYTES", "65536")?,
    };
    validate_secret_length(
        "BRIEFCASE_IAM_APP_SECRET",
        &settings.app_secret,
        MIN_SECRET_BYTES,
    )?;
    require_nonzero_duration("BRIEFCASE_IAM_CONNECT_TIMEOUT_MS", settings.connect_timeout)?;
    require_nonzero_duration("BRIEFCASE_IAM_REQUEST_TIMEOUT_MS", settings.request_timeout)?;
    if settings.connect_timeout >= settings.request_timeout {
        return Err(invalid(
            "BRIEFCASE_IAM_CONNECT_TIMEOUT_MS",
            "must be lower than BRIEFCASE_IAM_REQUEST_TIMEOUT_MS",
        ));
    }
    if settings.max_response_bytes.get() > MAX_IAM_RESPONSE_BYTES {
        return Err(invalid(
            "BRIEFCASE_IAM_MAX_RESPONSE_BYTES",
            "must not exceed 1048576 bytes",
        ));
    }
    Ok(settings)
}

fn s3_settings() -> Result<S3Settings, SettingsError> {
    let encryption_mode = value_or("BRIEFCASE_S3_ENCRYPTION_MODE", "sse_s3");
    let encryption = match encryption_mode.trim().to_ascii_lowercase().as_str() {
        "sse_s3" => {
            if optional("BRIEFCASE_S3_KMS_KEY_ARN").is_some() {
                return Err(invalid(
                    "BRIEFCASE_S3_KMS_KEY_ARN",
                    "must be omitted when encryption mode is sse_s3",
                ));
            }
            S3Encryption::SseS3
        }
        "sse_kms" => {
            let key_arn = required("BRIEFCASE_S3_KMS_KEY_ARN")?;
            if !is_kms_key_arn(&key_arn) {
                return Err(invalid(
                    "BRIEFCASE_S3_KMS_KEY_ARN",
                    "must be a full AWS KMS key ARN",
                ));
            }
            S3Encryption::SseKms { key_arn }
        }
        _ => {
            return Err(invalid(
                "BRIEFCASE_S3_ENCRYPTION_MODE",
                "must be sse_s3 or sse_kms",
            ));
        }
    };

    let bucket = required("BRIEFCASE_S3_BUCKET")?;
    validate_bucket_name(&bucket)?;
    let raw_key_prefix = required_or("BRIEFCASE_S3_KEY_PREFIX", "organizations")?;
    let key_prefix = normalized_key_prefix(&raw_key_prefix)?;
    let endpoint_url = optional("BRIEFCASE_S3_ENDPOINT_URL")
        .map(|value| parse("BRIEFCASE_S3_ENDPOINT_URL", &value))
        .transpose()?;
    if let Some(endpoint_url) = &endpoint_url {
        validate_service_url("BRIEFCASE_S3_ENDPOINT_URL", endpoint_url, false)?;
    }
    let temporary_directory = PathBuf::from(required_or(
        "BRIEFCASE_TEMPORARY_DIRECTORY",
        "/tmp/silicon-briefcase",
    )?);
    validate_temporary_directory(&temporary_directory)?;
    let settings = S3Settings {
        region: nonempty_value(
            "BRIEFCASE_S3_REGION",
            &value_or("BRIEFCASE_S3_REGION", "us-east-1"),
        )?,
        bucket,
        key_prefix,
        endpoint_url,
        force_path_style: parse_or("BRIEFCASE_S3_FORCE_PATH_STYLE", "false")?,
        encryption,
        temporary_directory,
        operation_timeout: duration_secs("BRIEFCASE_S3_OPERATION_TIMEOUT_SECONDS", 1_800)?,
    };
    require_nonzero_duration(
        "BRIEFCASE_S3_OPERATION_TIMEOUT_SECONDS",
        settings.operation_timeout,
    )?;
    Ok(settings)
}

fn webhook_settings() -> Result<WebhookSettings, SettingsError> {
    let settings = WebhookSettings {
        signing_secret: required_secret("BRIEFCASE_IAM_WEBHOOK_SIGNING_SECRET")?,
        signing_key_version: parse_or("BRIEFCASE_IAM_WEBHOOK_KEY_VERSION", "1")?,
        replay_window: duration_secs("BRIEFCASE_IAM_WEBHOOK_REPLAY_WINDOW_SECONDS", 300)?,
        max_body_bytes: parse_or("BRIEFCASE_IAM_WEBHOOK_MAX_BODY_BYTES", "262144")?,
    };
    validate_secret_length(
        "BRIEFCASE_IAM_WEBHOOK_SIGNING_SECRET",
        &settings.signing_secret,
        MIN_SECRET_BYTES,
    )?;
    require_nonzero_duration(
        "BRIEFCASE_IAM_WEBHOOK_REPLAY_WINDOW_SECONDS",
        settings.replay_window,
    )?;
    Ok(settings)
}

fn worker_settings() -> Result<WorkerSettings, SettingsError> {
    let settings = WorkerSettings {
        batch_size: parse_or("BRIEFCASE_WORKER_BATCH_SIZE", "100")?,
        cleanup_concurrency: parse_or("BRIEFCASE_WORKER_CLEANUP_CONCURRENCY", "4")?,
        poll_interval: duration_millis("BRIEFCASE_WORKER_POLL_INTERVAL_MS", 500)?,
        lease_duration: duration_secs("BRIEFCASE_WORKER_LEASE_SECONDS", 60)?,
        max_attempts: parse_or("BRIEFCASE_WORKER_MAX_ATTEMPTS", "20")?,
        max_retry_delay: duration_secs("BRIEFCASE_WORKER_MAX_RETRY_DELAY_SECONDS", 300)?,
        maintenance_interval: duration_secs("BRIEFCASE_WORKER_MAINTENANCE_INTERVAL_SECONDS", 60)?,
    };
    for (name, duration) in [
        ("BRIEFCASE_WORKER_POLL_INTERVAL_MS", settings.poll_interval),
        ("BRIEFCASE_WORKER_LEASE_SECONDS", settings.lease_duration),
        (
            "BRIEFCASE_WORKER_MAX_RETRY_DELAY_SECONDS",
            settings.max_retry_delay,
        ),
        (
            "BRIEFCASE_WORKER_MAINTENANCE_INTERVAL_SECONDS",
            settings.maintenance_interval,
        ),
    ] {
        require_nonzero_duration(name, duration)?;
    }
    if settings.poll_interval >= settings.lease_duration {
        return Err(invalid(
            "BRIEFCASE_WORKER_LEASE_SECONDS",
            "must be greater than the worker poll interval",
        ));
    }
    if settings.cleanup_concurrency > settings.batch_size {
        return Err(invalid(
            "BRIEFCASE_WORKER_CLEANUP_CONCURRENCY",
            "must not exceed BRIEFCASE_WORKER_BATCH_SIZE",
        ));
    }
    if settings.max_attempts == 0 {
        return Err(invalid(
            "BRIEFCASE_WORKER_MAX_ATTEMPTS",
            "must be greater than zero",
        ));
    }
    Ok(settings)
}

fn validate_environment_safety(
    environment: RuntimeEnvironment,
    server: &ServerSettings,
    database: &DatabaseSettings,
    iam: &IamSettings,
    s3: &S3Settings,
) -> Result<(), SettingsError> {
    if environment != RuntimeEnvironment::Production {
        return Ok(());
    }

    validate_database_transport(environment, database, "BRIEFCASE_DATABASE_URL")?;
    for (name, url) in [
        ("BRIEFCASE_PUBLIC_BASE_URL", &server.public_base_url),
        (
            "BRIEFCASE_PUBLIC_SITE_BASE_URL",
            &server.public_site_base_url,
        ),
        ("BRIEFCASE_IAM_BASE_URL", &iam.base_url),
    ] {
        require_https(name, url)?;
    }
    validate_storage_environment_safety(environment, s3)?;
    if server.bind_addr.ip().is_loopback() {
        return Err(invalid(
            "BRIEFCASE_BIND_ADDR",
            "must not bind only to loopback in production",
        ));
    }
    Ok(())
}

fn validate_storage_environment_safety(
    environment: RuntimeEnvironment,
    s3: &S3Settings,
) -> Result<(), SettingsError> {
    if environment != RuntimeEnvironment::Production {
        return Ok(());
    }
    if let Some(endpoint_url) = &s3.endpoint_url {
        require_https("BRIEFCASE_S3_ENDPOINT_URL", endpoint_url)?;
    }
    if !s3.temporary_directory.is_absolute() {
        return Err(invalid(
            "BRIEFCASE_TEMPORARY_DIRECTORY",
            "must be an absolute path in production",
        ));
    }
    Ok(())
}

fn validate_database_url(
    database: &DatabaseSettings,
    name: &'static str,
) -> Result<(), SettingsError> {
    let url = Url::parse(database.url.expose_secret())
        .map_err(|_| invalid(name, "must be a valid PostgreSQL URL"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(invalid(name, "must use the postgres:// scheme"));
    }
    if url.host_str().is_none() {
        return Err(invalid(name, "must include a database host"));
    }
    Ok(())
}

fn validate_database_transport(
    environment: RuntimeEnvironment,
    database: &DatabaseSettings,
    name: &'static str,
) -> Result<(), SettingsError> {
    if environment != RuntimeEnvironment::Production {
        return Ok(());
    }
    let url = Url::parse(database.url.expose_secret())
        .map_err(|_| invalid(name, "must be a valid PostgreSQL URL"))?;
    let ssl_mode = url
        .query_pairs()
        .find_map(|(key, value)| (key == "sslmode").then_some(value.into_owned()));
    if ssl_mode.as_deref() != Some("verify-full") {
        return Err(invalid(name, "must set sslmode=verify-full in production"));
    }
    Ok(())
}

fn validate_service_url(
    name: &'static str,
    url: &Url,
    permit_query: bool,
) -> Result<(), SettingsError> {
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid(name, "must be an absolute HTTP(S) URL"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(name, "must not contain embedded credentials"));
    }
    if url.fragment().is_some() || (!permit_query && url.query().is_some()) {
        return Err(invalid(name, "must not contain a query or fragment"));
    }
    Ok(())
}

fn normalized_base_url(name: &'static str, mut url: Url) -> Result<Url, SettingsError> {
    validate_service_url(name, &url, false)?;
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn join_endpoint(name: &'static str, base_url: &Url, path: &str) -> Result<Url, SettingsError> {
    let path = path.trim();
    if path.is_empty()
        || path.starts_with('/')
        || path.contains("..")
        || path.contains('?')
        || path.contains('#')
    {
        return Err(invalid(
            name,
            "must be a non-empty relative path without traversal or a query",
        ));
    }
    base_url
        .join(path)
        .map_err(|_| invalid(name, "cannot be resolved against the IAM base URL"))
}

fn require_https(name: &'static str, url: &Url) -> Result<(), SettingsError> {
    if url.scheme() != "https" {
        return Err(invalid(name, "must use https in production"));
    }
    Ok(())
}

fn validate_bucket_name(bucket: &str) -> Result<(), SettingsError> {
    let bytes = bucket.as_bytes();
    let has_valid_length = (3..=63).contains(&bytes.len());
    let has_valid_edges = bytes
        .first()
        .zip(bytes.last())
        .is_some_and(|(first, last)| first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric());
    let has_valid_characters = bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
    });
    let looks_like_ip = bucket.parse::<IpAddr>().is_ok();
    if !has_valid_length
        || !has_valid_edges
        || !has_valid_characters
        || bucket.contains("..")
        || looks_like_ip
    {
        return Err(invalid(
            "BRIEFCASE_S3_BUCKET",
            "must be a valid DNS-compatible S3 bucket name",
        ));
    }
    Ok(())
}

fn normalized_key_prefix(prefix: &str) -> Result<String, SettingsError> {
    let normalized = prefix.trim_matches('/').trim();
    let valid = !normalized.is_empty()
        && !normalized.contains("//")
        && !normalized.contains('\\')
        && !normalized.chars().any(char::is_control)
        && normalized
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."));
    if !valid {
        return Err(invalid(
            "BRIEFCASE_S3_KEY_PREFIX",
            "must be a safe, non-empty relative key prefix",
        ));
    }
    Ok(normalized.to_owned())
}

fn validate_temporary_directory(path: &Path) -> Result<(), SettingsError> {
    if path.as_os_str().is_empty() || path == Path::new("/") {
        return Err(invalid(
            "BRIEFCASE_TEMPORARY_DIRECTORY",
            "must identify a dedicated non-root directory",
        ));
    }
    Ok(())
}

fn is_kms_key_arn(value: &str) -> bool {
    let mut parts = value.split(':');
    matches!(parts.next(), Some("arn"))
        && parts.next().is_some()
        && matches!(parts.next(), Some("kms"))
        && parts.next().is_some_and(|region| !region.is_empty())
        && parts.next().is_some_and(|account| !account.is_empty())
        && parts
            .next()
            .is_some_and(|resource| resource.starts_with("key/"))
        && parts.next().is_none()
}

fn validate_secret_length(
    name: &'static str,
    value: &SecretString,
    minimum: usize,
) -> Result<(), SettingsError> {
    if value.expose_secret().len() < minimum {
        return Err(invalid(
            name,
            format!("must contain at least {minimum} bytes"),
        ));
    }
    Ok(())
}

fn require_nonzero_duration(name: &'static str, duration: Duration) -> Result<(), SettingsError> {
    if duration.is_zero() {
        return Err(invalid(name, "must be greater than zero"));
    }
    Ok(())
}

fn required(name: &'static str) -> Result<String, SettingsError> {
    optional(name).ok_or(SettingsError::Missing(name))
}

fn required_or(name: &'static str, default: &str) -> Result<String, SettingsError> {
    nonempty_value(name, &value_or(name, default))
}

fn required_secret(name: &'static str) -> Result<SecretString, SettingsError> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(SecretString::from)
        .ok_or(SettingsError::Missing(name))
}

fn optional(name: &'static str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn value_or(name: &'static str, default: &str) -> String {
    optional(name).unwrap_or_else(|| default.to_owned())
}

fn nonempty_value(name: &'static str, value: &str) -> Result<String, SettingsError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(invalid(name, "must contain 1-255 non-control characters"));
    }
    Ok(value.to_owned())
}

fn parse_or<T>(name: &'static str, default: &str) -> Result<T, SettingsError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let value = value_or(name, default);
    parse(name, &value)
}

fn parse<T>(name: &'static str, value: &str) -> Result<T, SettingsError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| invalid(name, error.to_string()))
}

fn duration_secs(name: &'static str, default: u64) -> Result<Duration, SettingsError> {
    Ok(Duration::from_secs(parse_or(name, &default.to_string())?))
}

fn duration_millis(name: &'static str, default: u64) -> Result<Duration, SettingsError> {
    Ok(Duration::from_millis(parse_or(name, &default.to_string())?))
}

fn invalid(name: &'static str, reason: impl Into<String>) -> SettingsError {
    SettingsError::Invalid {
        name,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeEnvironment, is_kms_key_arn, normalized_key_prefix};

    #[test]
    fn runtime_environment_is_case_insensitive() {
        assert_eq!(
            "PrOd".parse::<RuntimeEnvironment>(),
            Ok(RuntimeEnvironment::Production)
        );
    }

    #[test]
    fn key_prefix_rejects_path_traversal() {
        assert!(normalized_key_prefix("orgs/../other").is_err());
        assert_eq!(
            normalized_key_prefix("/orgs/objects/").ok(),
            Some("orgs/objects".to_owned())
        );
    }

    #[test]
    fn kms_key_arn_must_identify_a_key() {
        assert!(is_kms_key_arn(
            "arn:aws:kms:ap-south-1:123456789012:key/00000000-0000-0000-0000-000000000000"
        ));
        assert!(!is_kms_key_arn(
            "arn:aws:kms:ap-south-1:123456789012:alias/example"
        ));
    }
}
