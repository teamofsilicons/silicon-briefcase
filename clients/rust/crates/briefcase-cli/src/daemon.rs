//! One background service per operating-system user, shared across Silicon homes.
//!
//! Only private local IPC carries registration paths. Authentication remains in
//! each home's existing credential store; no tokens enter service definitions.

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{cli::DaemonCommand, render::Output, run::CliError, state::StateDirectory, updater};

mod service;

const SOCKET: &str = "control.sock";
const IPC_TIMEOUT: Duration = Duration::from_secs(3);

use briefcase_client::daemon::Request;

#[derive(Default, Deserialize, Serialize)]
struct Registry {
    homes: BTreeSet<PathBuf>,
}

fn io(error: std::io::Error) -> CliError {
    CliError::Io {
        path: "Briefcase daemon".into(),
        source: error,
    }
}

pub(super) fn root() -> Result<PathBuf, CliError> {
    if let Some(value) = std::env::var_os("BRIEFCASE_DAEMON_HOME") {
        if value.is_empty() {
            return Err(CliError::Usage(
                "BRIEFCASE_DAEMON_HOME must be a directory".into(),
            ));
        }
        return Ok(PathBuf::from(value));
    }
    // Deliberately independent of SILICON_HOME: all agents owned by this OS
    // account share the service while retaining separate application state.
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".briefcase-daemon"))
        .ok_or_else(|| {
            CliError::Usage("set HOME or BRIEFCASE_DAEMON_HOME to locate the shared daemon".into())
        })
}

fn private_directory(path: &Path) -> Result<(), CliError> {
    std::fs::create_dir_all(path).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}

fn private_file(path: &Path, append: bool) -> Result<File, CliError> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .append(append)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path).map_err(io)
}

fn read_registry(root: &Path) -> Result<Registry, CliError> {
    match std::fs::read(root.join("homes.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
            CliError::Usage("daemon homes.json is malformed; repair it before restarting".into())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(error) => Err(io(error)),
    }
}

fn save_registry(root: &Path, registry: &Registry) -> Result<(), CliError> {
    use std::io::Write as _;
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(io)?;
    serde_json::to_writer(&mut file, registry)
        .map_err(|error| CliError::Usage(format!("cannot encode daemon registry: {error}")))?;
    file.flush().map_err(io)?;
    file.as_file().sync_all().map_err(io)?;
    file.persist(root.join("homes.json"))
        .map_err(|error| io(error.error))?;
    Ok(())
}

async fn request(request: &Request) -> Result<Value, CliError> {
    briefcase_client::daemon::request(&root()?, request)
        .await
        .map_err(|error| CliError::Usage(error.to_string()))
}

/// Registers the current state directory with an already running daemon.
pub async fn register() -> Result<Value, CliError> {
    let state = StateDirectory::locate()?;
    private_directory(state.path())?;
    request(&Request::Register {
        state: state.path().canonicalize().map_err(io)?,
    })
    .await
}

async fn start() -> Result<Value, CliError> {
    if request(&Request::Status).await.is_ok() {
        return register().await;
    }
    let root = root()?;
    private_directory(&root)?;
    let log = private_file(&root.join("daemon.log"), true)?;
    let mut child = Command::new(std::env::current_exe().map_err(io)?)
        .args(["daemon", "run"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(log)
        // Do not persist one-use login tokens into a long-lived process.
        .env_remove("BRIEFCASE_TOKEN").env_remove("BRIEFCASE_APP_SECRET")
        .env_remove("BRIEFCASE_TEST").env_remove("BRIEFCASE_IAM_APP_SECRET")
        .env_remove("BRIEFCASE_IAM_TEST_KEY").env_remove("BRIEFCASE_IAM_ENVIRONMENT_KEY")
        .spawn().map_err(io)?;
    for _ in 0..50 {
        if let Ok(response) = register().await {
            return Ok(response);
        }
        if let Some(status) = child.try_wait().map_err(io)? {
            // Another starter can have won the lock; use its live endpoint.
            if let Ok(response) = register().await {
                return Ok(response);
            }
            return Err(CliError::Usage(format!(
                "daemon exited with {status}; inspect {}",
                root.join("daemon.log").display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(CliError::Usage(format!(
        "daemon startup timed out; inspect {}",
        root.join("daemon.log").display()
    )))
}

/// Runs a lifecycle command without requiring an IAM login.
pub async fn command(command: DaemonCommand, output: Output) -> Result<(), CliError> {
    let value = match command {
        DaemonCommand::Run => return serve().await,
        DaemonCommand::Start => start().await?,
        DaemonCommand::Status => match request(&Request::Status).await {
            Ok(value) => value,
            Err(error) => json!({"running": false, "reason": error.to_string()}),
        },
        DaemonCommand::Stop => request(&Request::Stop).await?,
        DaemonCommand::Install => {
            if request(&Request::Status).await.is_ok() {
                request(&Request::Stop).await?;
                for _ in 0..50 {
                    if !root()?.join(SOCKET).exists() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                if root()?.join(SOCKET).exists() {
                    return Err(CliError::Usage(
                        "daemon is finishing an update; retry installation after it exits".into(),
                    ));
                }
            }
            // The supervisor owns persistence and restart after installation.
            service::install()?;
            start().await?
        }
        DaemonCommand::Uninstall => {
            service::uninstall()?;
            let _ = request(&Request::Stop).await;
            json!({"installed": false})
        }
    };
    output.json(&value);
    Ok(())
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)] // Keep service startup and shutdown in one lifecycle.
async fn serve() -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt as _;
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
        net::UnixListener,
    };

    let root = root()?;
    private_directory(&root)?;
    let lock = private_file(&root.join("daemon.lock"), false)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        CliError::Usage(
            "a Briefcase daemon already owns this runtime; use `briefcase daemon status`".into(),
        )
    })?;
    let socket_path = root.join(SOCKET);
    match std::fs::remove_file(&socket_path) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(io(error)),
    }
    let listener = UnixListener::bind(&socket_path).map_err(io)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    let mut registry = read_registry(&root)?;
    let state = StateDirectory::locate()?;
    private_directory(state.path())?;
    registry
        .homes
        .insert(state.path().canonicalize().map_err(io)?);
    save_registry(&root, &registry)?;
    crate::telemetry::daemon_event(
        &state,
        "daemon_started",
        briefcase_client::telemetry::Stage::Started,
    )
    .await;
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut maintenance: Option<tokio::task::JoinHandle<()>> = None;
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(io)?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = interval.tick() => {
                if maintenance.as_ref().is_none_or(tokio::task::JoinHandle::is_finished) {
                    let homes = registry.homes.clone();
                    maintenance = Some(tokio::spawn(async move {
                        for home in homes {
                            // Missing homes are not recreated by maintenance.
                            if !home.is_dir() { continue; }
                            let state = StateDirectory::at(home);
                            let result = updater::automatic_for_state(&state).await;
                            if !matches!(result, Ok(updater::Outcome::Skipped)) {
                                crate::telemetry::daemon_event(&state, "update_check", if result.is_ok() { briefcase_client::telemetry::Stage::Completed } else { briefcase_client::telemetry::Stage::Failed }).await;
                            }
                            match result {
                                Ok(updater::Outcome::Updated { from, to }) => eprintln!("briefcase daemon: updated {from} to {to}"),
                                Err(error) => eprintln!("briefcase daemon: update check failed: {error}"),
                                _ => (),
                            }
                        }
                    }));
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(io)?;
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                let read = tokio::time::timeout(IPC_TIMEOUT, (&mut reader).take(8193).read_line(&mut line)).await;
                if !matches!(read, Ok(Ok(_))) || line.len() > 8192 { continue; }
                let mut stop = false;
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(Request::Status) => json!({"running": true, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "homes": registry.homes}),
                    Ok(Request::Stop) => { stop = true; json!({"running": false}) },
                    Ok(Request::Register { state }) => {
                        match state.canonicalize() {
                            Ok(path) if path.is_dir() => {
                                registry.homes.insert(path);
                                match save_registry(&root, &registry) {
                                    Ok(()) => json!({"running": true, "registered": true}),
                                    Err(error) => json!({"error": error.to_string()}),
                                }
                            },
                            _ => json!({"error": "registration requires an existing state directory"}),
                        }
                    }
                    Err(_) => json!({"error": "invalid daemon command"}),
                };
                let mut bytes = response.to_string().into_bytes(); bytes.push(b'\n');
                let _ = reader.get_mut().write_all(&bytes).await;
                if stop { break; }
            }
        }
    }
    // Finish any in-flight installer instead of leaving an orphan Cargo writer.
    if let Some(task) = maintenance {
        let _ = task.await;
    }
    std::fs::remove_file(socket_path).map_err(io)?;
    drop(lock);
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::too_many_lines)] // Keep service startup and shutdown in one lifecycle.
async fn serve() -> Result<(), CliError> {
    Err(CliError::Usage(
        "the shared daemon currently requires macOS or Linux".into(),
    ))
}
