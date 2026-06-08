//! Offline tests for `append` / `sync` (Phase 4 sub-mission 4.3).
//!
//! No live `nats-server`. Pins:
//!
//! * Detached test handle ([`RuntimeHandle::detached_for_tests`])
//!   surfaces a typed runtime error rather than silent no-op or
//!   panic (ADR-0022 §D7 "trap premature network call").
//! * [`JetStreamAckPosition`] is constructed only by the substrate;
//!   adopters compare via [`JetStreamAckPosition::as_u64`].
//!
//! Live-server tests live in `live_jetstream_*`, `#[ignore]`-gated.
use pardosa_nats::{JetStreamBackend, JetStreamConfig, JetStreamRuntimeError, RuntimeHandle};
fn detached_config(tag: &str) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(format!("offline-{tag}"))
        .subject(format!("offline.{tag}"))
        .durable_consumer(format!("offline-c-{tag}"))
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect("offline config is valid")
}
#[test]
fn append_with_detached_runtime_returns_detached_error() {
    let handle = JetStreamBackend::open(detached_config("append"));
    let err = handle
        .append(b"any-bytes")
        .expect_err("detached runtime cannot publish");
    assert!(
        matches!(err, JetStreamRuntimeError::Detached),
        "detached handle must surface Detached, got {err:?}"
    );
}
#[test]
fn sync_with_detached_runtime_returns_detached_error() {
    let handle = JetStreamBackend::open(detached_config("sync"));
    let err = handle.sync().expect_err("detached runtime cannot sync");
    assert!(
        matches!(err, JetStreamRuntimeError::Detached),
        "detached handle must surface Detached, got {err:?}"
    );
}
#[test]
fn replay_all_with_detached_runtime_returns_detached_error() {
    let handle = JetStreamBackend::open(detached_config("replay"));
    let err = handle
        .replay_all()
        .expect_err("detached runtime cannot replay");
    assert!(
        matches!(err, JetStreamRuntimeError::Detached),
        "detached handle must surface Detached, got {err:?}"
    );
}
