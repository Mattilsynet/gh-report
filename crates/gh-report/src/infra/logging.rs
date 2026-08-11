//! # Logging Convention
//!
//! All log sites use [`tracing`], per these rules, for Cloud Logging
//! compatibility.
//!
//! ## Messages
//!
//! Static strings only — never `{}` interpolate; variable data goes in
//! `key = value` fields. Lowercase, no trailing punctuation, 2–5 words.
//! Past participle when done, present participle in-progress.
//!
//! ## Structured Fields
//!
//! `snake_case`, emitted as-is in JSON mode. Canonical vocabulary: `repo`,
//! `repos`, `org`, `error`, `path`, `run_id`, `status`, `attempt`. Use `%`
//! (Display) for errors, except `JoinError` which uses `?` (Debug) to
//! preserve panic payload; `?` otherwise for non-`Display` types.
//!
//! ## Severity
//!
//! `error!` unrecoverable; `warn!` degraded/integrity; `info!`
//! lifecycle; `debug!` troubleshooting; `trace!` per-item.
//!
//! ## GCP Cloud Logging
//!
//! In JSON mode, [`CloudLoggingLayer`] (see
//! [`cloud_logging`](super::cloud_logging)) emits Cloud Run JSON, mapping
//! tracing levels to `severity` (`DEBUG`/`TRACE` map to `"DEBUG"`).
//! Example:
//!
//! ```json
//! {"severity":"INFO","message":"baseline loaded","time":"2026-04-13T12:34:51.775621Z","target":"gh_report::infra::baseline","entries":560}
//! ```
//!
//! ## Runtime-adjustable level (CHE-0079, adr-fmt-pq1b6.1.2)
//!
//! [`build_reloadable_filter`] wraps the startup [`EnvFilter`] in a
//! [`reload::Layer`], returning a [`LogReloadHandle`] the caller retains.
//! [`LogReloadHandle::set_directive`] swaps the active filter without a
//! process restart. Per CHE-0079 (No Bespoke Ops Console), this crate ships
//! only the reload primitive — no hosted console; a caller wires it to
//! whatever minimal signal or config mechanism the deployment prefers.

use tracing_subscriber::filter::ParseError;
use tracing_subscriber::{EnvFilter, Registry, reload};

/// Runtime handle over an active [`EnvFilter`] layered atop a [`Registry`].
///
/// Cloning is cheap (the underlying [`reload::Handle`] is an `Arc`).
#[derive(Clone, Debug)]
pub struct LogReloadHandle(reload::Handle<EnvFilter, Registry>);

/// Failure modes for [`LogReloadHandle::set_directive`].
#[derive(Debug)]
#[non_exhaustive]
pub enum LogReloadError {
    /// The supplied directive string failed to parse as an `EnvFilter`.
    InvalidDirective {
        /// The directive string that failed to parse.
        directive: String,
        /// The underlying parse failure.
        source: ParseError,
    },
    /// The reload could not be applied because the subscriber was dropped.
    SubscriberGone,
}

impl std::fmt::Display for LogReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDirective { directive, source } => {
                write!(f, "invalid log directive {directive:?}: {source}")
            }
            Self::SubscriberGone => {
                write!(f, "log filter subscriber no longer exists")
            }
        }
    }
}

impl std::error::Error for LogReloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidDirective { source, .. } => Some(source),
            Self::SubscriberGone => None,
        }
    }
}

impl LogReloadHandle {
    /// Replaces the active filter with one parsed from `directive`
    /// (the same syntax as `RUST_LOG`, e.g. `"info,gh_report=debug"`).
    ///
    /// # Errors
    ///
    /// Returns [`LogReloadError::InvalidDirective`] when `directive` fails
    /// to parse, and [`LogReloadError::SubscriberGone`] when the wrapped
    /// subscriber has already been dropped.
    pub fn set_directive(&self, directive: &str) -> Result<(), LogReloadError> {
        let filter =
            EnvFilter::try_new(directive).map_err(|source| LogReloadError::InvalidDirective {
                directive: directive.to_string(),
                source,
            })?;
        self.0
            .reload(filter)
            .map_err(|_| LogReloadError::SubscriberGone)
    }
}

/// Builds a [`reload::Layer`] wrapping an [`EnvFilter`] parsed from
/// `directive`, plus the [`LogReloadHandle`] used to change it at runtime.
///
/// # Errors
///
/// Returns [`LogReloadError::InvalidDirective`] when `directive` fails to
/// parse as an `EnvFilter` directive string.
pub fn build_reloadable_filter(
    directive: &str,
) -> Result<(reload::Layer<EnvFilter, Registry>, LogReloadHandle), LogReloadError> {
    let filter =
        EnvFilter::try_new(directive).map_err(|source| LogReloadError::InvalidDirective {
            directive: directive.to_string(),
            source,
        })?;
    let (layer, handle) = reload::Layer::new(filter);
    Ok((layer, LogReloadHandle(handle)))
}

/// Spawns a background task that reloads the log filter from the current
/// `RUST_LOG` environment value whenever the process receives `SIGHUP`.
///
/// This is the minimal signal mechanism CHE-0079 calls for in place of a
/// bespoke ops console: operators change verbosity with
/// `RUST_LOG=<directive> kill -HUP <pid>` (or an orchestrator equivalent),
/// never through a hosted control surface. A no-op on non-Unix platforms,
/// where no `SIGHUP` exists.
pub fn spawn_sighup_reload_listener(handle: LogReloadHandle) {
    #[cfg(unix)]
    {
        tokio::spawn(async move {
            let Ok(mut sighup) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            else {
                tracing::warn!("SIGHUP handler install failed, log reload unavailable");
                return;
            };
            loop {
                sighup.recv().await;
                let directive = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
                match handle.set_directive(&directive) {
                    Ok(()) => {
                        tracing::info!(directive = %directive, "log level reloaded via SIGHUP");
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, directive = %directive, "log level reload rejected");
                    }
                }
            }
        });
    }
    #[cfg(not(unix))]
    {
        let _ = handle;
    }
}

#[cfg(test)]
type Recording = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone)]
    struct RecordingLayer(Recording);

    impl<S> tracing_subscriber::Layer<S> for RecordingLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(event.metadata().name().to_string());
        }
    }

    fn emit_at_all_levels() {
        tracing::error!(target: "gh_report::infra::logging::tests", "e");
        tracing::warn!(target: "gh_report::infra::logging::tests", "w");
        tracing::info!(target: "gh_report::infra::logging::tests", "i");
        tracing::debug!(target: "gh_report::infra::logging::tests", "d");
        tracing::trace!(target: "gh_report::infra::logging::tests", "t");
    }

    #[test]
    fn startup_directive_gates_events_by_level() {
        let (filter_layer, _handle) =
            build_reloadable_filter("gh_report::infra::logging::tests=warn").unwrap();
        let recorded: Recording = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = Registry::default()
            .with(filter_layer)
            .with(RecordingLayer(recorded.clone()));

        tracing::subscriber::with_default(subscriber, emit_at_all_levels);

        let names = recorded.lock().unwrap();
        assert_eq!(
            names.len(),
            2,
            "expected only error+warn to pass a warn-level filter, got {names:?}"
        );
    }

    #[test]
    fn reload_handle_changes_effective_level_at_runtime() {
        let (filter_layer, handle) =
            build_reloadable_filter("gh_report::infra::logging::tests=error").unwrap();
        let recorded: Recording = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = Registry::default()
            .with(filter_layer)
            .with(RecordingLayer(recorded.clone()));

        tracing::subscriber::with_default(subscriber, || {
            emit_at_all_levels();
            assert_eq!(
                recorded.lock().unwrap().len(),
                1,
                "startup directive should admit error only before reload"
            );

            handle
                .set_directive("gh_report::infra::logging::tests=trace")
                .unwrap();

            emit_at_all_levels();
        });

        assert_eq!(
            recorded.lock().unwrap().len(),
            1 + 5,
            "post-reload directive should admit all five levels"
        );
    }

    #[test]
    fn set_directive_rejects_invalid_syntax() {
        let (_layer, handle) = build_reloadable_filter("info").unwrap();
        let err = handle.set_directive("not a valid==directive").unwrap_err();
        assert!(matches!(err, LogReloadError::InvalidDirective { .. }));
    }

    #[test]
    fn build_reloadable_filter_rejects_invalid_startup_directive() {
        let err = build_reloadable_filter("not a valid==directive").unwrap_err();
        assert!(matches!(err, LogReloadError::InvalidDirective { .. }));
    }
}
