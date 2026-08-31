//! Cross-platform process shutdown coordination.

/// Waits for `SIGINT` or `SIGTERM`, logs the signal, and then resolves.
///
/// On non-Unix platforms only `SIGINT` is observed. A failed signal-handler
/// installation is logged and does not cause a production process to stop
/// immediately.
pub async fn signal() {
    let ctrl_c = async {
        match tokio::signal::ctrl_c().await {
            Ok(()) => tracing::info!(signal = "SIGINT", "shutdown signal received"),
            Err(error) => {
                tracing::error!(?error, "failed to install SIGINT handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
                tracing::info!(signal = "SIGTERM", "shutdown signal received");
            }
            Err(error) => {
                tracing::error!(?error, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
