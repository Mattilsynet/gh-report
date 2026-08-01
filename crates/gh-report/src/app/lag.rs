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

use crate::config::{COLLECTION_INTERVAL_SECS, PROJECTION_LAG_EVENT_CRITICAL, PROJECTION_LAG_EVENT_WARNING};

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

    #[must_use]
    pub(crate) const fn seq_lag(&self) -> u64 {
        self.writer_head_seq.saturating_sub(self.projection_applied_seq)
    }

    #[must_use]
    pub(crate) const fn time_lag_secs(&self) -> u64 {
        self.writer_head_at_millis
            .saturating_sub(self.projection_applied_at_millis)
            / 1000
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
