//! Graceful shutdown signalling.

/// Resolves on `SIGINT` (Ctrl-C) or, on Unix, `SIGTERM`.
///
/// `SIGTERM` is what container orchestrators send before a `SIGKILL`, so
/// handling it is what lets in-flight requests finish during a rolling deploy.
pub async fn signal() {
    let interrupt = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => tracing::info!("received SIGINT, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
