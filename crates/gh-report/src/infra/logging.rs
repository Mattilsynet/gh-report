//! # Logging Convention
//!
//! All log sites use the [`tracing`] framework and must follow these rules
//! for consistency and GCP Cloud Logging compatibility.
//!
//! ## Messages
//!
//! - **Static strings only** — never use `{}` interpolation in the message.
//!   All variable data goes into structured `key = value` fields.
//! - Lowercase, no trailing punctuation.
//! - Past participle for completed events ("loaded", "saved", "rendered").
//! - Present participle for in-progress actions ("processing", "collecting").
//! - Keep messages short (2–5 words). Fields provide context.
//!
//! ## Structured Fields
//!
//! - `snake_case` names in source code. In JSON mode, field names are emitted
//!   as-is in `snake_case`.
//! - Canonical vocabulary:
//!   - `repo` — repository name
//!   - `repos` — repository count
//!   - `org` — organization
//!   - `error` — error value (use `%` / Display format)
//!   - `path` — filesystem or URL path
//!   - `run_id` — collection run identifier
//!   - `status` — HTTP or evaluation status
//!   - `attempt` — retry attempt number
//! - Use `%` (Display) for error fields at all log levels.
//! - **Exception:** `tokio::task::JoinError` uses `?` (Debug) to preserve
//!   the panic payload for diagnosis.
//! - Use `?` (Debug) only for types lacking a `Display` impl (e.g.,
//!   `Option<T>`).
//!
//! ## Severity Guidelines
//!
//! - `error!` — unrecoverable failures requiring operator attention.
//! - `warn!`  — degraded behavior, fallback activated, data integrity
//!   concerns.
//! - `info!`  — significant lifecycle events visible in production.
//! - `debug!` — detailed operational info for troubleshooting.
//! - `trace!` — per-item granularity.
//!
//! ## GCP Cloud Logging
//!
//! In JSON mode (`--log-format json`), the custom [`CloudLoggingLayer`]
//! (see [`cloud_logging`](super::cloud_logging)) outputs structured JSON
//! compatible with Cloud Logging on Cloud Run:
//!
//! | tracing level | Cloud Logging `severity` |
//! |---------------|--------------------------|
//! | `ERROR`       | `"ERROR"`                |
//! | `WARN`        | `"WARNING"`              |
//! | `INFO`        | `"INFO"`                 |
//! | `DEBUG`       | `"DEBUG"`                |
//! | `TRACE`       | `"DEBUG"`                |
//!
//! Example output:
//!
//! ```json
//! {"severity":"INFO","message":"baseline loaded","time":"2026-04-13T12:34:51.775621Z","target":"gh_report::infra::baseline","entries":560}
//! ```
