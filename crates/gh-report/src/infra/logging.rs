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
