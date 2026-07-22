//! Single-writer cell reconciling CHE-0048:R7's in-process write lock with
//! COM-0018:R3's snapshot-read requirement (CHE-0096).

use std::sync::{Arc, Mutex};

use cherry_pit_core::{EventEnvelope, Projection};

/// Single-writer cell over a [`Projection`].
///
/// Writes serialize through an in-process lock (CHE-0048:R7); the lock is
/// never exposed on the public surface. Reads return an owned snapshot
/// (COM-0018:R3), never a lock guard. The cell does not synthesize event
/// identity — callers supply a fully constructed [`EventEnvelope`]
/// (CHE-0096:R3).
pub struct WriteCell<P: Projection> {
    inner: Arc<Mutex<P>>,
}

impl<P: Projection> WriteCell<P> {
    /// Construct a cell seeded with `P::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(P::default())),
        }
    }

    /// Fold `envelope` into the projection under the write lock.
    ///
    /// The caller constructs `envelope` (sequence, aggregate id, timestamp);
    /// this method does not synthesize event identity (CHE-0096:R3).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned, matching the single-writer
    /// invariant convention established at the `AppState::lock_projection`
    /// call site (CHE-0048).
    pub fn apply(&self, envelope: &EventEnvelope<P::Event>) {
        let mut guard = self.inner.lock().expect("WriteCell mutex poisoned");
        guard.apply(envelope);
    }

    /// Replace the projection state wholesale (resync swap).
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    pub fn replace(&self, projection: P) {
        let mut guard = self.inner.lock().expect("WriteCell mutex poisoned");
        *guard = projection;
    }
}

impl<P: Projection> Default for WriteCell<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Projection + Clone> WriteCell<P> {
    /// Read an immutable, independently-owned snapshot of the projection
    /// (COM-0018:R3). Never returns a lock guard; mutating the returned
    /// value has no effect on the cell.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> P {
        let guard = self.inner.lock().expect("WriteCell mutex poisoned");
        guard.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
    use serde::{Deserialize, Serialize};
    use std::num::NonZeroU64;
    use std::sync::Arc as StdArc;
    use std::thread;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented,
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct CounterView {
        total: u64,
    }

    impl Projection for CounterView {
        type Event = CounterEvent;

        fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {
            self.total += 1;
        }
    }

    fn envelope(seq: u64) -> EventEnvelope<CounterEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(seq).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            CounterEvent::Incremented,
        )
        .expect("valid envelope")
    }

    #[test]
    fn concurrent_apply_serializes_no_lost_update() {
        let cell = StdArc::new(WriteCell::<CounterView>::new());
        let threads: Vec<_> = (1..=50u64)
            .map(|seq| {
                let cell = StdArc::clone(&cell);
                thread::spawn(move || cell.apply(&envelope(seq)))
            })
            .collect();
        for t in threads {
            t.join().expect("writer thread panicked");
        }
        assert_eq!(cell.snapshot().total, 50);
    }

    #[test]
    fn snapshot_is_independent_owned_value() {
        let cell = WriteCell::<CounterView>::new();
        cell.apply(&envelope(1));
        let mut snap = cell.snapshot();
        assert_eq!(snap.total, 1);

        snap.total = 999;
        assert_eq!(snap.total, 999, "the local mutation itself did take effect");
        assert_eq!(
            cell.snapshot().total,
            1,
            "mutating a returned snapshot must not affect the cell"
        );

        cell.apply(&envelope(2));
        assert_eq!(
            cell.snapshot().total,
            2,
            "a fresh snapshot after apply() observes the new state"
        );
    }
}
