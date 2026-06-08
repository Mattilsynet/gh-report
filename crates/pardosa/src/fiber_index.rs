//! In-memory `FiberIndex` core (ADR-0023 D1, D4, D5, D6).
//!
//! Per-journal, log-derived, application-keyed routing accelerator.
//! Closure-first zero-to-many extractor (D1); lookup returns
//! [`FiberLookup::Empty`] / [`FiberLookup::Unique`] /
//! [`FiberLookup::Diverged`] (D4); `K` is application-owned and
//! opaque to pardosa (D6). No persistence — log is the sole
//! durable artefact (D2/D5). Re-exported from
//! [`crate::store`] (D5 default-public surface).
use crate::event::{Event, FiberId};
use std::collections::HashMap;
use std::hash::Hash;
/// Typed lookup result for [`FiberIndex::lookup`] (ADR-0023 D4).
///
/// `Diverged` is a typed value, not a failure: the substrate does
/// not own domain identity and so never decides which fiber is
/// "correct" for a colliding `K`. `Diverged.fibers` is in
/// first-observation order over the log (stable under rebuild,
/// per D3). The enum is `#[non_exhaustive]` per ADR-0007.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FiberLookup<F> {
    /// `K` never observed up to the index's catch-up point.
    Empty,
    /// `K` observed on exactly one fiber.
    Unique(F),
    /// `K` observed on two or more distinct fibers within this
    /// journal. `fibers` is in first-observation log order.
    Diverged {
        /// All fibers `K` was observed on, in first-observation
        /// log order.
        fibers: Vec<F>,
    },
}
/// Typed extractor-side error surface for [`FiberIndex::try_build`]
/// / [`FiberIndex::try_observe`] (ADR-0023 D4 final clause).
///
/// `#[non_exhaustive]` per ADR-0007 so downstream `match` arms
/// without a wildcard do not compile and new variants may land
/// without a breaking change. Carries the offending `EventId` and
/// `FiberId` per D4 so the caller can name the failing event.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExtractError {
    /// Adopter-supplied extractor rejected the event.
    #[error("extractor rejected event {event_id} on fiber {fiber_id}: {source}")]
    Extractor {
        /// Offending event id.
        event_id: crate::event::EventId,
        /// Offending fiber id.
        fiber_id: FiberId,
        /// Adopter-supplied cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}
/// In-memory `K → fibers` routing accelerator over a single
/// journal (ADR-0023 D1).
///
/// Build via [`FiberIndex::build`] (canonical closure-first
/// happy path, zero-to-many `K` per event) or
/// [`FiberIndex::try_build`] (fallible extractor). Update
/// incrementally via [`FiberIndex::observe`] /
/// [`FiberIndex::try_observe`]. Look up via
/// [`FiberIndex::lookup`].
///
/// `K`: application-owned, opaque to pardosa (D6). Mechanical
/// bounds only: `Hash + Eq + Clone` for in-process storage.
pub struct FiberIndex<K> {
    map: HashMap<K, Vec<FiberId>>,
}
impl<K> Default for FiberIndex<K>
where
    K: Hash + Eq + Clone,
{
    fn default() -> Self {
        Self::empty()
    }
}
impl<K> FiberIndex<K>
where
    K: Hash + Eq + Clone,
{
    /// An empty index. Suitable for [`FiberIndex::observe`]-driven
    /// incremental construction.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
    /// Build the index by replaying `events` through the
    /// closure-first zero-to-many `extractor` (ADR-0023 D1).
    ///
    /// `extractor` may return any [`IntoIterator`] of `K` per
    /// event: zero `K` yields no mapping; one yields one
    /// `K → fiber`; many yields one mapping per emitted `K`.
    /// Pure function of the supplied events under the extractor
    /// (D3 determinism / log-replay equivalence).
    pub fn build<T, F, I>(events: &[Event<T>], extractor: F) -> Self
    where
        F: Fn(&Event<T>) -> I,
        I: IntoIterator<Item = K>,
    {
        let mut idx = Self::empty();
        for event in events {
            idx.observe(event, &extractor);
        }
        idx
    }
    /// Fallible variant of [`FiberIndex::build`] (ADR-0023 D4
    /// extractor error surface).
    ///
    /// Stops at the first `Err`, naming the offending event in
    /// [`ExtractError::Extractor`]. The partially-built index is
    /// discarded so the caller never observes a silently-
    /// inconsistent index.
    ///
    /// # Errors
    ///
    /// [`ExtractError::Extractor`] when the extractor returns
    /// `Err` for any event in `events`.
    pub fn try_build<T, F, I, E>(events: &[Event<T>], extractor: F) -> Result<Self, ExtractError>
    where
        F: Fn(&Event<T>) -> Result<I, E>,
        I: IntoIterator<Item = K>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut idx = Self::empty();
        for event in events {
            idx.try_observe(event, &extractor)?;
        }
        Ok(idx)
    }
    /// Apply `extractor` to a single `event` and merge the
    /// resulting `K → fiber` mappings into the index (ADR-0023 D1
    /// append-side update).
    ///
    /// Repeated calls with the same `(K, fiber)` pair are
    /// idempotent: the index keeps the first observation and
    /// does not duplicate. A new fiber for an already-observed
    /// `K` appends to the per-`K` vector in log order, so the
    /// next [`FiberIndex::lookup`] surfaces a
    /// [`FiberLookup::Diverged`] with both fibers.
    pub fn observe<T, F, I>(&mut self, event: &Event<T>, extractor: F)
    where
        F: Fn(&Event<T>) -> I,
        I: IntoIterator<Item = K>,
    {
        let fiber_id = event.fiber_id();
        for k in extractor(event) {
            self.insert_one(k, fiber_id);
        }
    }
    /// Fallible variant of [`FiberIndex::observe`] (ADR-0023 D4).
    ///
    /// # Errors
    ///
    /// [`ExtractError::Extractor`] when the extractor returns
    /// `Err`. The index is left unchanged for the failing event
    /// (no silent partial update).
    pub fn try_observe<T, F, I, E>(
        &mut self,
        event: &Event<T>,
        extractor: F,
    ) -> Result<(), ExtractError>
    where
        F: Fn(&Event<T>) -> Result<I, E>,
        I: IntoIterator<Item = K>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let fiber_id = event.fiber_id();
        let event_id = event.event_id();
        let iter = extractor(event).map_err(|e| ExtractError::Extractor {
            event_id,
            fiber_id,
            source: Box::new(e),
        })?;
        for k in iter {
            self.insert_one(k, fiber_id);
        }
        Ok(())
    }
    fn insert_one(&mut self, k: K, fiber_id: FiberId) {
        let entry = self.map.entry(k).or_default();
        if !entry.contains(&fiber_id) {
            entry.push(fiber_id);
        }
    }
    /// Look up `k`'s fiber mapping. Returns one of the three
    /// typed shapes pinned by ADR-0023 D4.
    #[must_use]
    pub fn lookup(&self, k: &K) -> FiberLookup<FiberId> {
        match self.map.get(k) {
            None => FiberLookup::Empty,
            Some(fibers) if fibers.is_empty() => FiberLookup::Empty,
            Some(fibers) if fibers.len() == 1 => FiberLookup::Unique(fibers[0]),
            Some(fibers) => FiberLookup::Diverged {
                fibers: fibers.clone(),
            },
        }
    }
    /// Number of distinct `K` values currently indexed.
    #[must_use]
    pub fn key_count(&self) -> usize {
        self.map.len()
    }
}
impl<K> std::fmt::Debug for FiberIndex<K>
where
    K: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FiberIndex")
            .field("key_count", &self.map.len())
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventId, Index, Precursor};
    fn ev(eid: u64, fid: u64, payload: &str) -> Event<String> {
        Event::new_unchecked(
            eid,
            FiberId::new(fid),
            false,
            if eid == 0 {
                Precursor::Genesis
            } else {
                Precursor::Of(Index::new(eid - 1))
            },
            [0u8; 32],
            payload.to_string(),
        )
    }
    #[test]
    fn empty_index_lookup_returns_empty() {
        let idx: FiberIndex<String> = FiberIndex::empty();
        assert_eq!(idx.lookup(&"never".to_string()), FiberLookup::Empty);
        assert_eq!(idx.key_count(), 0);
    }
    #[test]
    fn build_extractor_zero_yields_no_mapping() {
        let events = vec![ev(0, 1, "ignored"), ev(1, 1, "ignored")];
        let idx: FiberIndex<String> = FiberIndex::build(&events, |_| Vec::<String>::new());
        assert_eq!(idx.key_count(), 0);
        assert_eq!(idx.lookup(&"any".to_string()), FiberLookup::Empty);
    }
    #[test]
    fn build_extractor_one_yields_unique() {
        let events = vec![ev(0, 7, "k0")];
        let idx: FiberIndex<String> =
            FiberIndex::build(&events, |e| vec![e.domain_event().clone()]);
        assert_eq!(idx.key_count(), 1);
        assert_eq!(
            idx.lookup(&"k0".to_string()),
            FiberLookup::Unique(FiberId::new(7))
        );
    }
    #[test]
    fn build_extractor_many_emits_one_mapping_per_k() {
        let events = vec![ev(0, 3, "a,b,c")];
        let idx: FiberIndex<String> = FiberIndex::build(&events, |e| {
            e.domain_event()
                .split(',')
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        assert_eq!(idx.key_count(), 3);
        assert_eq!(
            idx.lookup(&"a".to_string()),
            FiberLookup::Unique(FiberId::new(3))
        );
        assert_eq!(
            idx.lookup(&"b".to_string()),
            FiberLookup::Unique(FiberId::new(3))
        );
        assert_eq!(
            idx.lookup(&"c".to_string()),
            FiberLookup::Unique(FiberId::new(3))
        );
    }
    #[test]
    fn divergence_records_all_fibers_in_log_order() {
        let events = vec![ev(0, 1, "shared"), ev(1, 2, "shared"), ev(2, 3, "shared")];
        let idx: FiberIndex<String> =
            FiberIndex::build(&events, |e| vec![e.domain_event().clone()]);
        let look = idx.lookup(&"shared".to_string());
        match look {
            FiberLookup::Diverged { fibers } => {
                assert_eq!(
                    fibers,
                    vec![FiberId::new(1), FiberId::new(2), FiberId::new(3)]
                );
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }
    #[test]
    fn divergence_transitions_unique_to_diverged_on_observe() {
        let mut idx: FiberIndex<String> = FiberIndex::empty();
        idx.observe(&ev(0, 1, "k"), |e| vec![e.domain_event().clone()]);
        assert_eq!(
            idx.lookup(&"k".to_string()),
            FiberLookup::Unique(FiberId::new(1))
        );
        idx.observe(&ev(1, 2, "k"), |e| vec![e.domain_event().clone()]);
        match idx.lookup(&"k".to_string()) {
            FiberLookup::Diverged { fibers } => {
                assert_eq!(fibers, vec![FiberId::new(1), FiberId::new(2)]);
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }
    #[test]
    fn repeated_observation_of_same_pair_is_idempotent() {
        let mut idx: FiberIndex<String> = FiberIndex::empty();
        let event = ev(0, 1, "k");
        for _ in 0..5 {
            idx.observe(&event, |e| vec![e.domain_event().clone()]);
        }
        assert_eq!(idx.key_count(), 1);
        assert_eq!(
            idx.lookup(&"k".to_string()),
            FiberLookup::Unique(FiberId::new(1))
        );
    }
    #[test]
    fn determinism_two_builds_same_events_yield_same_lookup() {
        let events = vec![ev(0, 1, "a"), ev(1, 2, "a"), ev(2, 1, "b")];
        let extractor = |e: &Event<String>| vec![e.domain_event().clone()];
        let a: FiberIndex<String> = FiberIndex::build(&events, extractor);
        let b: FiberIndex<String> = FiberIndex::build(&events, extractor);
        assert_eq!(a.lookup(&"a".to_string()), b.lookup(&"a".to_string()));
        assert_eq!(a.lookup(&"b".to_string()), b.lookup(&"b".to_string()));
        assert_eq!(a.key_count(), b.key_count());
    }
    #[test]
    fn try_build_propagates_extractor_error() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope at {0}")]
        struct Bad(u64);
        let events = vec![ev(0, 1, "ok"), ev(1, 1, "fail")];
        let result: Result<FiberIndex<String>, ExtractError> =
            FiberIndex::try_build(&events, |e| {
                if e.domain_event() == "fail" {
                    Err(Bad(e.event_id().value()))
                } else {
                    Ok(vec![e.domain_event().clone()])
                }
            });
        let err = result.expect_err("must propagate");
        match err {
            ExtractError::Extractor {
                event_id,
                fiber_id,
                source: _,
            } => {
                assert_eq!(event_id, EventId::new(1));
                assert_eq!(fiber_id, FiberId::new(1));
            }
        }
    }
    #[test]
    fn try_observe_does_not_silently_skip_on_error() {
        #[derive(Debug, thiserror::Error)]
        #[error("nope")]
        struct Bad;
        let mut idx: FiberIndex<String> = FiberIndex::empty();
        let err = idx
            .try_observe(&ev(0, 1, "k"), |_| Err::<Vec<String>, _>(Bad))
            .expect_err("must propagate");
        assert!(matches!(err, ExtractError::Extractor { .. }));
        assert_eq!(idx.key_count(), 0);
    }
}
