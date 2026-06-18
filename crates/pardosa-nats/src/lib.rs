//! JetStream-backed authoritative-storage substrate for pardosa
//! (ADR-0022 §D10, §D11).
//!
//! Substrate-ring sibling to `pardosa-file`; depends on
//! `tokio`, `async-nats`, `bytes`, `blake3`, and `futures-util`.
//! The `blake3` dependency supports duplicate suppression through
//! deterministic `Nats-Msg-Id` values. Exports the opaque
//! [`JetStreamHandle`] and constructor [`JetStreamBackend::open`].
//! Sealed `AuthoritativeBackend` / `BackendSink` impls live in
//! `pardosa` as an in-crate adapter shim.
//!
//! `JetStreamBackend::open` does not contact the server;
//! connection and stream provisioning happen on the first
//! `append` / `sync`.
//!
//! Phase 1.5 bindings enforced at construction: single-subject
//! (no `*`/`>`), `Discard: New`, `Storage: File`, `R ≥ 1`.
#![forbid(unsafe_code)]
mod config;
mod error;
mod handle;
mod runtime;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub use config::{
    DEFAULT_OPERATION_TIMEOUT, Discard, JetStreamConfig, JetStreamConfigBuilder,
    OPERATION_TIMEOUT_ENV, Storage,
};
pub use error::{JetStreamConfigError, JetStreamRuntimeError};
pub use handle::{JetStreamAckPosition, JetStreamBackend, JetStreamHandle, JetStreamReplayRecord};
pub use runtime::RuntimeHandle;
