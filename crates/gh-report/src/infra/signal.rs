//! Shared shutdown signal listener.
//!
//! Extracts the platform-specific signal handling logic into a single
//! reusable function for both server shutdown and background task
//! cancellation.
//!
//! Absorbed under mission `absorb-helpers-1778694900` (P1-A.5.1).
//! Byte-for-byte port from prior upstream helpers.

use tracing::warn;

/// Wait for a shutdown signal (SIGINT/Ctrl-C or SIGTERM).
///
/// On Unix, listens for both `SIGINT` (Ctrl-C) and `SIGTERM`. If the
/// `SIGTERM` handler fails to install, falls back to `SIGINT` only.
///
/// On non-Unix platforms, listens for Ctrl-C only.
///
/// Returns when either signal is received. Callers are responsible for
/// their own shutdown logic (releasing locks, cancelling tokens, etc.).
pub async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                warn!(error = %e, "SIGTERM handler install failed, using ctrl-c only");
                if let Err(e) = tokio::signal::ctrl_c().await {
                    warn!(error = %e, "failed to listen for ctrl-c");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!(error = %e, "failed to listen for ctrl-c");
        }
    }
}
