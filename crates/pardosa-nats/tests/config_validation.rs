//! Config validation tests (ADR-0022 §D10/§D11; Phase 1.5 §7.3).
//!
//! Offline shape: config validation, opaque-handle construction,
//! typed errors. No live server contacted —
//! `JetStreamBackend::open` does not reach the network.
use pardosa_nats::{
    DEFAULT_OPERATION_TIMEOUT, Discard, JetStreamBackend, JetStreamConfig, JetStreamConfigError,
    OPERATION_TIMEOUT_ENV, RuntimeHandle, Storage,
};
use std::time::Duration;
fn valid_config() -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect("valid config builds")
}
#[test]
fn builder_accepts_valid_config() {
    let cfg = valid_config();
    assert_eq!(cfg.stream_name(), "PARDOSA_TEST");
    assert_eq!(cfg.subject(), "pardosa.events.test");
    assert_eq!(cfg.durable_consumer(), "pardosa-cursor-test");
    assert_eq!(cfg.storage(), Storage::File);
    assert_eq!(cfg.discard(), Discard::New);
    assert_eq!(cfg.replicas().get(), 1);
    assert_eq!(cfg.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
}
#[test]
fn builder_rejects_empty_stream_name() {
    let err = JetStreamConfig::builder()
        .stream_name("")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("empty stream name must be rejected");
    assert!(matches!(err, JetStreamConfigError::EmptyStreamName));
}
#[test]
fn builder_rejects_empty_subject() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("empty subject must be rejected");
    assert!(matches!(err, JetStreamConfigError::EmptySubject));
}
#[test]
fn builder_rejects_wildcard_subject_star() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.*")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("wildcard '*' must be rejected (Phase 1.5 §7 single subject)");
    assert!(matches!(
        err,
        JetStreamConfigError::SubjectContainsWildcard { .. }
    ));
}
#[test]
fn builder_rejects_wildcard_subject_greater_than() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.>")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("wildcard '>' must be rejected (Phase 1.5 §7 single subject)");
    assert!(matches!(
        err,
        JetStreamConfigError::SubjectContainsWildcard { .. }
    ));
}
#[test]
fn builder_rejects_empty_durable_consumer() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("empty durable-consumer name must be rejected");
    assert!(matches!(err, JetStreamConfigError::EmptyDurableConsumer));
}
#[test]
fn builder_rejects_discard_old_per_phase_1_5_7_3() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .discard(Discard::Old)
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("Discard::Old is forbidden in v0 (Phase 1.5 §7.3)");
    assert!(matches!(err, JetStreamConfigError::DiscardOldForbidden));
}
#[test]
fn builder_rejects_replicas_zero() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .replicas(0)
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .build()
        .expect_err("replicas = 0 is invalid (Phase 1.5 §7.3 R≥1)");
    assert!(matches!(err, JetStreamConfigError::ReplicasMustBePositive));
}
#[test]
fn builder_requires_runtime_handle() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .build()
        .expect_err("missing runtime handle must be rejected (ADR-0022 §D7)");
    assert!(matches!(err, JetStreamConfigError::MissingRuntimeHandle));
}
#[test]
fn builder_accepts_operation_timeout_override() {
    let cfg = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .operation_timeout(Duration::from_mins(2))
        .build()
        .expect("timeout override builds");
    assert_eq!(cfg.operation_timeout(), Duration::from_mins(2));
}
#[test]
fn builder_rejects_zero_operation_timeout() {
    let err = JetStreamConfig::builder()
        .stream_name("PARDOSA_TEST")
        .subject("pardosa.events.test")
        .durable_consumer("pardosa-cursor-test")
        .runtime_handle(RuntimeHandle::detached_for_tests())
        .operation_timeout(Duration::ZERO)
        .build()
        .expect_err("zero timeout must be rejected");
    assert!(matches!(
        err,
        JetStreamConfigError::OperationTimeoutMustBePositive
    ));
}
#[test]
fn operation_timeout_env_name_is_stable() {
    assert_eq!(OPERATION_TIMEOUT_ENV, "PARDOSA_NATS_OPERATION_TIMEOUT_SECS");
}
#[test]
fn open_does_not_touch_network() {
    let cfg = valid_config();
    let handle = JetStreamBackend::open(cfg);
    let _ = handle;
}
#[test]
fn handle_is_constructible_from_path_like_factory() {
    let cfg = valid_config();
    let _handle: pardosa_nats::JetStreamHandle = JetStreamBackend::open(cfg);
}
