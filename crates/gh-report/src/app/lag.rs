//! Projection-lag observability and enforcement (PGN-0023 R1/R6, Amendment
//! 2026-08-01 dual-form ratified ceiling).
//!
//! [`LagCounters`] tracks `writer_head_seq` and `projection_applied_seq`
//! (PGN-0023:R1) from the same in-process choke point every write goes
//! through today ([`crate::app::state::AppState::record_repo`] and
//! siblings, which call the native store `record`/`detach` then fold
//! into the projection back-to-back). Under the single-daemon topology
//! this keeps [`LagSnapshot::seq_lag`] and
//! [`LagSnapshot::time_lag_secs`] near zero without any topology check
//! in this module — the near-zero result falls out of both counters
//! advancing at the same call site. After the serving/writing split
//! (#8), a writer process and a serving process update these counters
//! independently and real lag becomes observable with no change here.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::{
    COLLECTION_INTERVAL_SECS, PROJECTION_LAG_EVENT_CRITICAL, PROJECTION_LAG_EVENT_WARNING,
};

/// Time-based WARNING threshold: 1x `COLLECTION_INTERVAL_SECS` (CHE-0104),
/// per PGN-0023 Amendment 2026-08-01.
pub(crate) const LAG_TIME_WARNING_SECS: u64 = COLLECTION_INTERVAL_SECS;

/// Time-based CRITICAL threshold: 2x `COLLECTION_INTERVAL_SECS`.
pub(crate) const LAG_TIME_CRITICAL_SECS: u64 = COLLECTION_INTERVAL_SECS * 2;

/// Runtime counters backing the `writer_head_seq - projection_applied_seq`
/// primitive (PGN-0023:R1).
#[derive(Debug, Default)]
pub(crate) struct LagCounters {
    writer_head_seq: AtomicU64,
    writer_head_at_millis: AtomicU64,
    projection_applied_seq: AtomicU64,
    projection_applied_at_millis: AtomicU64,
}

impl LagCounters {
    pub(crate) fn record_write(&self) {
        self.writer_head_seq.fetch_add(1, Ordering::SeqCst);
        self.writer_head_at_millis
            .store(now_millis(), Ordering::SeqCst);
    }

    pub(crate) fn record_applied(&self) {
        self.projection_applied_seq.fetch_add(1, Ordering::SeqCst);
        self.projection_applied_at_millis
            .store(now_millis(), Ordering::SeqCst);
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> LagSnapshot {
        LagSnapshot {
            writer_head_seq: self.writer_head_seq.load(Ordering::SeqCst),
            writer_head_at_millis: self.writer_head_at_millis.load(Ordering::SeqCst),
            projection_applied_seq: self.projection_applied_seq.load(Ordering::SeqCst),
            projection_applied_at_millis: self.projection_applied_at_millis.load(Ordering::SeqCst),
        }
    }
}

fn now_millis() -> u64 {
    u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap_or(u64::MAX)
}

/// Point-in-time read of [`LagCounters`], the dual-form primitive
/// PGN-0023:R1 and its 2026-08-01 amendment enforce against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LagSnapshot {
    writer_head_seq: u64,
    writer_head_at_millis: u64,
    projection_applied_seq: u64,
    projection_applied_at_millis: u64,
}

impl LagSnapshot {
    #[cfg(test)]
    pub(crate) const fn for_test(
        writer_head_seq: u64,
        writer_head_at_millis: u64,
        projection_applied_seq: u64,
        projection_applied_at_millis: u64,
    ) -> Self {
        Self {
            writer_head_seq,
            writer_head_at_millis,
            projection_applied_seq,
            projection_applied_at_millis,
        }
    }

    /// Rebind `writer_head_seq` to a cross-process-sourced value
    /// (PGN-0027 R1), leaving `projection_applied_seq` and the
    /// time-lag fields untouched: those stay same-process facts.
    #[must_use]
    pub(crate) const fn with_writer_head_seq(self, writer_head_seq: u64) -> Self {
        Self {
            writer_head_seq,
            ..self
        }
    }

    #[must_use]
    pub(crate) const fn seq_lag(&self) -> u64 {
        self.writer_head_seq
            .saturating_sub(self.projection_applied_seq)
    }

    #[must_use]
    pub(crate) const fn time_lag_secs(&self) -> u64 {
        self.writer_head_at_millis
            .saturating_sub(self.projection_applied_at_millis)
            / 1000
    }
}

/// Source of the `writer_head_seq` component of a lag snapshot
/// (PGN-0027). Type-driven (R16): the topology choice made once at
/// `AppState` construction is a value, not a runtime flag re-checked on
/// every read — a `Local` process can never accidentally take the
/// cross-process branch and vice versa.
#[derive(Default)]
#[allow(
    dead_code,
    reason = "CrossProcess is constructed by the future #8 split-process serving startup path (PGN-0027 risks/migration: no code ships with the Draft ADR) and by this file's own tests; #[expect] would be unfulfilled on a --tests build where the test module already constructs it"
)]
pub(crate) enum WriterHeadSource {
    /// Single-daemon or split-collector (PGN-0027 §Context): same as
    /// today, `writer_head_seq` comes from the same-process
    /// [`LagCounters`] atomic.
    #[default]
    Local,
    /// Split-serving process (PGN-0027 R1): `writer_head_seq` is read
    /// from the `JetStream` stream head through the typed read-side port.
    CrossProcess(Box<pardosa::head::WriterHeadReader>),
}

impl WriterHeadSource {
    /// Resolve the current `writer_head_seq`. `Local` reads
    /// `local`'s same-process counter unchanged. `CrossProcess` reads
    /// the `JetStream` stream head (PGN-0027 R1); on a transient read
    /// failure it falls back to `local`'s last-known value rather than
    /// refusing, per R3/R4 (the read tier never gates on this read).
    #[must_use]
    pub(crate) fn resolve(&self, local: &LagCounters) -> u64 {
        match self {
            Self::Local => local.snapshot().writer_head_seq,
            Self::CrossProcess(reader) => reader
                .writer_head_seq()
                .unwrap_or_else(|_| local.snapshot().writer_head_seq),
        }
    }
}

/// PGN-0023 R1/Amendment 2026-08-01 severity classification: illegal
/// combinations (e.g. "critical but no measurement") are unrepresentable
/// — the payload is only ever attached to `Warning`/`Critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LagSeverity {
    /// Neither form has crossed its WARNING threshold.
    Nominal,
    /// One or both forms crossed WARNING but neither crossed CRITICAL:
    /// observable tripwire only, read still serves (Amendment 2026-08-01).
    Warning { seq_lag: u64, time_lag_secs: u64 },
    /// One or both forms crossed CRITICAL: read must refuse (PGN-0023:R1
    /// RYW-safety intent, unconditional per the amendment).
    Critical { seq_lag: u64, time_lag_secs: u64 },
}

impl LagSeverity {
    #[must_use]
    pub(crate) fn classify(snapshot: LagSnapshot) -> Self {
        let seq_lag = snapshot.seq_lag();
        let time_lag_secs = snapshot.time_lag_secs();
        if seq_lag >= PROJECTION_LAG_EVENT_CRITICAL || time_lag_secs > LAG_TIME_CRITICAL_SECS {
            return Self::Critical {
                seq_lag,
                time_lag_secs,
            };
        }
        if seq_lag >= PROJECTION_LAG_EVENT_WARNING || time_lag_secs > LAG_TIME_WARNING_SECS {
            return Self::Warning {
                seq_lag,
                time_lag_secs,
            };
        }
        Self::Nominal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detached_head_reader() -> pardosa::head::WriterHeadReader {
        let cfg = pardosa_nats::JetStreamConfig::builder()
            .stream_name("gh-report-lag-test")
            .subject("gh-report.lag-test.events")
            .durable_consumer("gh-report-lag-test")
            .nats_url("nats://127.0.0.1:4222")
            .runtime_handle(pardosa_nats::RuntimeHandle::detached_for_tests())
            .build()
            .expect("minimal config builds");
        pardosa::head::WriterHeadReader::open(pardosa_nats::JetStreamBackend::open(cfg))
    }

    #[test]
    fn local_source_resolves_from_same_process_counters() {
        let counters = LagCounters::default();
        counters.record_write();
        counters.record_write();

        assert_eq!(WriterHeadSource::Local.resolve(&counters), 2);
    }

    #[test]
    fn writer_head_source_defaults_to_local() {
        assert!(matches!(
            WriterHeadSource::default(),
            WriterHeadSource::Local
        ));
    }

    #[test]
    fn cross_process_source_falls_back_to_local_on_read_failure() {
        let counters = LagCounters::default();
        counters.record_write();
        counters.record_write();
        counters.record_write();
        let source = WriterHeadSource::CrossProcess(Box::new(detached_head_reader()));

        assert_eq!(
            source.resolve(&counters),
            3,
            "detached reader cannot reach a live stream; read tier falls back rather than refuses"
        );
    }

    #[test]
    fn with_writer_head_seq_rebinds_only_the_seq_component() {
        let snapshot = LagSnapshot::for_test(1, 1_000, 1, 900).with_writer_head_seq(9);

        assert_eq!(snapshot.seq_lag(), 8);
        assert_eq!(
            snapshot.time_lag_secs(),
            LagSnapshot::for_test(1, 1_000, 1, 900).time_lag_secs()
        );
    }

    #[test]
    fn near_zero_lag_classifies_nominal() {
        let snapshot = LagSnapshot::for_test(42, 1_000, 42, 1_000);
        assert_eq!(LagSeverity::classify(snapshot), LagSeverity::Nominal);
    }

    #[test]
    fn event_count_warning_threshold_trips_warning_not_critical() {
        let snapshot = LagSnapshot::for_test(142, 1_000, 42, 1_000);
        assert_eq!(
            LagSeverity::classify(snapshot),
            LagSeverity::Warning {
                seq_lag: 100,
                time_lag_secs: 0
            }
        );
    }

    #[test]
    fn event_count_critical_threshold_trips_critical() {
        let snapshot = LagSnapshot::for_test(542, 1_000, 42, 1_000);
        assert_eq!(
            LagSeverity::classify(snapshot),
            LagSeverity::Critical {
                seq_lag: 500,
                time_lag_secs: 0
            }
        );
    }

    #[test]
    fn time_warning_threshold_trips_warning_not_critical() {
        let writer_at = 1_000 + (LAG_TIME_WARNING_SECS + 1) * 1000;
        let snapshot = LagSnapshot::for_test(1, writer_at, 1, 1_000);
        assert_eq!(
            LagSeverity::classify(snapshot),
            LagSeverity::Warning {
                seq_lag: 0,
                time_lag_secs: LAG_TIME_WARNING_SECS + 1
            }
        );
    }

    #[test]
    fn time_critical_threshold_trips_critical() {
        let writer_at = 1_000 + (LAG_TIME_CRITICAL_SECS + 1) * 1000;
        let snapshot = LagSnapshot::for_test(1, writer_at, 1, 1_000);
        assert_eq!(
            LagSeverity::classify(snapshot),
            LagSeverity::Critical {
                seq_lag: 0,
                time_lag_secs: LAG_TIME_CRITICAL_SECS + 1
            }
        );
    }

    #[test]
    fn record_write_and_record_applied_back_to_back_stay_near_zero_lag() {
        let counters = LagCounters::default();
        counters.record_write();
        counters.record_applied();
        counters.record_write();
        counters.record_applied();
        let snapshot = counters.snapshot();
        assert_eq!(
            LagSeverity::classify(snapshot),
            LagSeverity::Nominal,
            "single-daemon write-then-fold call site must not falsely trip the gate"
        );
    }

    #[test]
    fn record_write_without_matching_apply_grows_seq_lag() {
        let counters = LagCounters::default();
        for _ in 0..PROJECTION_LAG_EVENT_CRITICAL {
            counters.record_write();
        }
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.seq_lag(), PROJECTION_LAG_EVENT_CRITICAL);
        assert!(matches!(
            LagSeverity::classify(snapshot),
            LagSeverity::Critical { seq_lag, .. } if seq_lag == PROJECTION_LAG_EVENT_CRITICAL
        ));
    }
}
