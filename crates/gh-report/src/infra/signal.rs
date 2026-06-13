//! Shared shutdown signal listener.
//!
//! Extracts the platform-specific signal handling logic into a single
//! reusable function for both server shutdown and background task
//! cancellation.
//!
//! Absorbed under mission `absorb-helpers-1778694900` (P1-A.5.1).
//! Byte-for-byte port from prior upstream helpers.

use tracing::warn;

/// Shutdown signal identity observed by the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    Terminate,
    Interrupt,
}

impl ShutdownSignal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminate => "SIGTERM",
            Self::Interrupt => "SIGINT",
        }
    }
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalSource {
    Terminate,
    Interrupt,
}

#[cfg(any(unix, test))]
fn shutdown_signal_from_source(source: SignalSource) -> ShutdownSignal {
    match source {
        SignalSource::Terminate => ShutdownSignal::Terminate,
        SignalSource::Interrupt => ShutdownSignal::Interrupt,
    }
}

const fn fallback_shutdown_signal() -> ShutdownSignal {
    ShutdownSignal::Interrupt
}

/// Wait for a shutdown signal (SIGINT/Ctrl-C or SIGTERM).
///
/// On Unix, listens for both `SIGINT` (Ctrl-C) and `SIGTERM`. If the
/// `SIGTERM` handler fails to install, falls back to `SIGINT` only.
///
/// On non-Unix platforms, listens for Ctrl-C only.
///
/// Returns when either signal is received. Callers are responsible for
/// their own shutdown logic (releasing locks, cancelling tokens, etc.).
pub async fn wait_for_shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                let source = tokio::select! {
                    _ = tokio::signal::ctrl_c() => SignalSource::Interrupt,
                    _ = sigterm.recv() => SignalSource::Terminate,
                };
                shutdown_signal_from_source(source)
            }
            Err(e) => {
                warn!(error = %e, "SIGTERM handler install failed, using ctrl-c only");
                if let Err(e) = tokio::signal::ctrl_c().await {
                    warn!(error = %e, "failed to listen for ctrl-c");
                }
                fallback_shutdown_signal()
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!(error = %e, "failed to listen for ctrl-c");
        }
        fallback_shutdown_signal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sigterm_source_to_terminate() {
        assert_eq!(
            shutdown_signal_from_source(SignalSource::Terminate),
            ShutdownSignal::Terminate
        );
    }

    #[test]
    fn maps_interrupt_source_to_interrupt() {
        assert_eq!(
            shutdown_signal_from_source(SignalSource::Interrupt),
            ShutdownSignal::Interrupt
        );
    }

    #[test]
    fn fallback_signal_is_interrupt() {
        assert_eq!(fallback_shutdown_signal(), ShutdownSignal::Interrupt);
    }

    #[test]
    fn signal_names_match_operational_signals() {
        assert_eq!(ShutdownSignal::Terminate.as_str(), "SIGTERM");
        assert_eq!(ShutdownSignal::Interrupt.as_str(), "SIGINT");
    }
}
