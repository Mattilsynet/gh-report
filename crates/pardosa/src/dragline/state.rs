use super::linevec::Linevec;
#[cfg(test)]
use crate::error::{FromRawPartsKind, PardosaError};
use crate::event::Event;
#[cfg(test)]
use crate::event::Index;
use crate::event::{EventId, FiberId};
use crate::fiber::Fiber;
use crate::fiber_state::FiberState;
use crate::frontier::{AnchorInterval, Frontier};
#[cfg(test)]
use pardosa_wire::Encode;
use std::collections::{HashMap, HashSet};
pub(crate) const DEFAULT_ANCHOR_INTERVAL: u64 = 1_000;
/// Default cap on the publish-anchor buffer (ADR-0010 — durability gating).
/// Overflow surfaces as a typed `PardosaError::AnchorBufferOverflow`, not a
/// panic and not silent loss. Large enough that real adopters won't hit it
/// during a single `sync_data` window; small enough to catch a runaway
/// producer that never syncs.
pub(crate) const DEFAULT_ANCHOR_BUFFER_CAP: usize = 65_536;
fn default_anchor_interval() -> AnchorInterval {
    AnchorInterval::try_new(DEFAULT_ANCHOR_INTERVAL).expect("DEFAULT_ANCHOR_INTERVAL is non-zero")
}
#[derive(Debug, Clone, Copy)]
#[must_use]
#[non_exhaustive]
pub(crate) struct AppendResult {
    pub fiber_id: FiberId,
    pub event_id: EventId,
}
#[derive(Debug)]
#[allow(clippy::struct_field_names)]
pub(crate) struct Line<T> {
    pub(super) line: Linevec<T>,
    pub(super) lookup: HashMap<FiberId, (Fiber, FiberState)>,
    pub(super) purged_ids: HashSet<FiberId>,
    pub(super) next_id: FiberId,
    pub(super) next_event_id: EventId,
    pub(super) migrating: bool,
    pub(super) frontier: Frontier,
    pub(super) stream_name: String,
    pub(super) anchor_interval: AnchorInterval,
    pub(super) events_since_tick: u64,
    /// Anchors buffered between `sync_data` calls (ADR-0010 publish-after-
    /// durable gating). Drained by `Dragline::sync_data` post-fsync.
    /// `None` ⇒ no publisher attached at this layer; anchors are dropped
    /// silently. `Some(buf)` ⇒ commits append `(subject, payload)` to `buf`
    /// at each anchor-interval tick, bounded by `anchor_buffer_cap`.
    pub(super) pending_anchors: Option<Vec<(String, [u8; 32])>>,
    /// Cap on `pending_anchors.len()` — overflow returns
    /// `PardosaError::AnchorBufferOverflow` from the offending
    /// `commit_event`. Inert when `pending_anchors` is `None`.
    pub(super) anchor_buffer_cap: usize,
}
impl<T> Default for Line<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Line<T> {
    /// Build an empty `Line` with default anchor interval and no frontier publisher.
    #[must_use]
    pub fn new() -> Self {
        Line {
            line: Linevec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: FiberId::from_decoded(0),
            next_event_id: EventId::ZERO,
            migrating: false,
            frontier: Frontier::GENESIS,
            stream_name: String::new(),
            anchor_interval: default_anchor_interval(),
            events_since_tick: 0,
            pending_anchors: None,
            anchor_buffer_cap: DEFAULT_ANCHOR_BUFFER_CAP,
        }
    }
    /// Configure anchor cadence and stream name without attaching a publisher.
    /// Anchors fired during `create`/`update`/`detach` are buffered into
    /// `pending_anchors` and drained by `Dragline::sync_data` (publish-after-
    /// durable gating; ADR-0010). The `Line` itself is publisher-agnostic
    /// in SM-3 — the dispatch surface moved up to `Dragline`.
    ///
    /// `anchor_buffer_cap` bounds the buffer; zero is permitted but turns
    /// every anchor tick into an overflow. Use `DEFAULT_ANCHOR_BUFFER_CAP`
    /// (`65_536`) for production adopters.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with_anchor_config(
        stream_name: String,
        anchor_interval: u64,
        anchor_buffer_cap: usize,
    ) -> Self {
        Line {
            line: Linevec::new(),
            lookup: HashMap::new(),
            purged_ids: HashSet::new(),
            next_id: FiberId::from_decoded(0),
            next_event_id: EventId::ZERO,
            migrating: false,
            frontier: Frontier::GENESIS,
            stream_name,
            anchor_interval: AnchorInterval::new_or_one(anchor_interval),
            events_since_tick: 0,
            pending_anchors: Some(Vec::new()),
            anchor_buffer_cap,
        }
    }
    /// The rolling BLAKE3 frontier — a 32-byte tamper-evident commitment over
    /// the full event line. `Frontier::GENESIS` before any events have been
    /// committed; advances on every `commit`.
    #[must_use]
    pub fn frontier(&self) -> Frontier {
        self.frontier
    }
    /// Drain the publish-anchor buffer. Returns an empty `Vec` when no
    /// publisher was attached at this layer (i.e. `pending_anchors` is
    /// `None`). In-tree test-only since the in-process publisher seam
    /// was deleted (mission `invariant-port-09-20260602`); the reachable
    /// publisher path (`JournalMode::Durable`) reconstructs anchors
    /// from the persisted line gated by the publish-watermark sidecar
    /// (ADR-0016 §D6).
    #[cfg(test)]
    pub(crate) fn drain_pending_anchors(&mut self) -> Vec<(String, [u8; 32])> {
        match self.pending_anchors.as_mut() {
            Some(buf) => std::mem::take(buf),
            None => Vec::new(),
        }
    }
    /// M2 (roadmap correctness 2): check whether this `Line` carries
    /// state that cannot round-trip through a `.pgno` event line.
    /// Called by `persist::persist_with_source` as a preflight before any byte hits
    /// the sink. Returns `Ok(())` on persistable state, otherwise the
    /// first unpersistable reason encountered (order: `migrating` →
    /// `purged_ids` → `Locked` lookup entry).
    pub(crate) fn check_persistable(&self) -> Result<(), crate::persist::UnpersistableKind> {
        use crate::persist::UnpersistableKind;
        if self.migrating {
            return Err(UnpersistableKind::Migrating);
        }
        if !self.purged_ids.is_empty() {
            return Err(UnpersistableKind::PurgedIdsNonEmpty);
        }
        for (fiber_id, (_fiber, state)) in &self.lookup {
            if *state == FiberState::Locked {
                return Err(UnpersistableKind::LockedLookupEntry {
                    fiber_id: *fiber_id,
                });
            }
        }
        Ok(())
    }
    /// Rebuild a `Line` from persisted parts and verify all
    /// invariants.
    ///
    /// Re-folds frontier from `line` and rejects any `supplied`
    /// value that disagrees; closes ADR-0004 §3's
    /// caller-supplied-frontier surface for in-process callers.
    /// Normal code uses [`Line::new`] /
    /// [`Line::with_anchor_config`] and the incremental commit
    /// API. The `pub(crate) from_parts_unchecked` fast-path remains
    /// for in-crate test fixtures.
    ///
    /// # Errors
    ///
    /// - [`PardosaError::FrontierMismatch`] — `supplied` ≠ re-fold.
    /// - Any variant `verify_invariants` returns (see
    ///   [`from_parts_unchecked`](Self::from_parts_unchecked)).
    #[cfg(test)]
    pub(crate) fn from_raw_parts(
        line: Vec<Event<T>>,
        lookup: HashMap<FiberId, (Fiber, FiberState)>,
        purged_ids: HashSet<FiberId>,
        next_id: FiberId,
        next_event_id: EventId,
        migrating: bool,
        frontier: Frontier,
    ) -> Result<Self, PardosaError>
    where
        T: Encode,
    {
        let computed = line.iter().fold(Frontier::GENESIS, |acc, e| {
            acc.roll(&pardosa_wire::to_vec(e))
        });
        if computed != frontier {
            return Err(PardosaError::FrontierMismatch {
                supplied: frontier,
                computed,
            });
        }
        if migrating {
            return Err(PardosaError::FromRawParts(FromRawPartsKind::Migrating));
        }
        if !purged_ids.is_empty() {
            return Err(PardosaError::FromRawParts(
                FromRawPartsKind::PurgedIdsNonEmpty,
            ));
        }
        verify_supplied_against_canonical(&line, &lookup, next_id, next_event_id)?;
        Self::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            migrating,
            computed,
        )
    }
    /// Rebuild a `Line` from persisted parts without re-folding
    /// frontier. Trusts the caller-supplied `frontier`; runs
    /// structural invariant checks only.
    ///
    /// Reserved for in-crate callers that already folded frontier
    /// during replay (`persist::rehydrate_unchecked`) and in-crate
    /// tests that deliberately construct invalid lines for
    /// tamper-injection. Public surface is
    /// [`Line::from_raw_parts`] (ADR-0004 §3).
    ///
    /// # Errors
    ///
    /// `verify_invariants` rejects with `PardosaError::FiberInvariant
    /// (Integrity(_))` for `EventIdNotMonotonic`, `PurgedIdInLookup`,
    /// `NextEventIdMismatch`, `FiberCurrentOutOfBounds`, or
    /// `PardosaError::BrokenPrecursorChain` for non-contiguous
    /// precursor linkage within a fiber.
    #[cfg(test)]
    pub(crate) fn from_parts_unchecked(
        line: Vec<Event<T>>,
        lookup: HashMap<FiberId, (Fiber, FiberState)>,
        purged_ids: HashSet<FiberId>,
        next_id: FiberId,
        next_event_id: EventId,
        migrating: bool,
        frontier: Frontier,
    ) -> Result<Self, PardosaError>
    where
        T: Encode,
    {
        let d = Line {
            line: Linevec::from_raw_unchecked(line),
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            migrating,
            frontier,
            stream_name: String::new(),
            anchor_interval: default_anchor_interval(),
            events_since_tick: 0,
            pending_anchors: None,
            anchor_buffer_cap: DEFAULT_ANCHOR_BUFFER_CAP,
        };
        d.verify_invariants()?;
        Ok(d)
    }
    /// Reader-rebuild fast-path for the rehydrate pipeline
    /// (ADR-0020 reader bound). Trusts `frontier` and the
    /// structural invariants of `(line, lookup, next_id,
    /// next_event_id)`; caller has walked the line and rolled
    /// the frontier (ADR-0004 §1).
    ///
    /// Skips `verify_invariants` — that path requires `T: Encode`.
    /// Checked/validated rehydrate paths validate the precursor
    /// chain while streaming.
    pub(crate) fn from_parts_no_verify(
        line: Vec<Event<T>>,
        lookup: HashMap<FiberId, (Fiber, FiberState)>,
        purged_ids: HashSet<FiberId>,
        next_id: FiberId,
        next_event_id: EventId,
        migrating: bool,
        frontier: Frontier,
    ) -> Self {
        Line {
            line: Linevec::from_raw_unchecked(line),
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            migrating,
            frontier,
            stream_name: String::new(),
            anchor_interval: default_anchor_interval(),
            events_since_tick: 0,
            pending_anchors: None,
            anchor_buffer_cap: DEFAULT_ANCHOR_BUFFER_CAP,
        }
    }
}
/// M4 (roadmap correctness 4) — canonical-state cross-check for
/// `Line::from_raw_parts`.
///
/// Walks `line` in position order to derive the canonical
/// `(lookup, next_id, next_event_id)` set, then compares against the
/// supplied values. Returns the first disagreement as a typed
/// [`FromRawPartsKind`] wrapped in
/// [`PardosaError::FromRawParts`]. Empty `line` collapses to
/// `next_id == 0`, `next_event_id == 0`, empty lookup — all enforced.
///
/// Does **not** validate event-id contiguity or precursor chains; those
/// remain the responsibility of `verify_invariants` /
/// `verify_precursor_chains` (run by `from_parts_unchecked` immediately
/// after this helper).
#[cfg(test)]
fn verify_supplied_against_canonical<T>(
    line: &[Event<T>],
    supplied_lookup: &HashMap<FiberId, (Fiber, FiberState)>,
    supplied_next_id: FiberId,
    supplied_next_event_id: EventId,
) -> Result<(), PardosaError>
where
    T: Encode,
{
    let mut canonical: HashMap<FiberId, (Index, u64, Index, FiberState)> = HashMap::new();
    let mut max_fiber_id: Option<FiberId> = None;
    for (i, event) in line.iter().enumerate() {
        let position = Index::from_decoded(u64::try_from(i).expect("line position fits u64"));
        let fid = event.fiber_id();
        max_fiber_id = Some(match max_fiber_id {
            None => fid,
            Some(prev) if fid.value() > prev.value() => fid,
            Some(prev) => prev,
        });
        let state = if event.detached() {
            FiberState::Detached
        } else {
            FiberState::Defined
        };
        canonical
            .entry(fid)
            .and_modify(|(_anchor, count, current, st)| {
                *count += 1;
                *current = position;
                *st = state;
            })
            .or_insert((position, 1, position, state));
    }
    let canonical_next_id = match max_fiber_id {
        None => FiberId::from_decoded(0),
        Some(m) => m.checked_next()?,
    };
    let canonical_next_event_id =
        EventId::from_decoded(u64::try_from(line.len()).expect("line.len() fits u64"));
    if supplied_next_id != canonical_next_id {
        return Err(PardosaError::FromRawParts(
            FromRawPartsKind::NextIdMismatch {
                supplied: supplied_next_id,
                expected: canonical_next_id,
            },
        ));
    }
    if supplied_next_event_id != canonical_next_event_id {
        return Err(PardosaError::FromRawParts(
            FromRawPartsKind::NextEventIdMismatch {
                supplied: supplied_next_event_id,
                expected: canonical_next_event_id,
            },
        ));
    }
    for (fid, (_anchor, _count, expected_current, expected_state)) in &canonical {
        let (sfiber, sstate) = supplied_lookup.get(fid).ok_or(PardosaError::FromRawParts(
            FromRawPartsKind::LookupMissingFiber { fiber_id: *fid },
        ))?;
        if sfiber.current() != *expected_current {
            return Err(PardosaError::FromRawParts(
                FromRawPartsKind::FiberCurrentMismatch {
                    fiber_id: *fid,
                    supplied: sfiber.current(),
                    expected: *expected_current,
                },
            ));
        }
        if *sstate != *expected_state {
            return Err(PardosaError::FromRawParts(
                FromRawPartsKind::FiberStateMismatch {
                    fiber_id: *fid,
                    supplied: *sstate,
                    expected: *expected_state,
                },
            ));
        }
    }
    for fid in supplied_lookup.keys() {
        if !canonical.contains_key(fid) {
            return Err(PardosaError::FromRawParts(
                FromRawPartsKind::LookupExtraFiber { fiber_id: *fid },
            ));
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{FiberInvariantKind, IntegrityKind};
    use crate::event::Precursor;
    use crate::{Event, EventId, Fiber, FiberId, FiberState, Index, PardosaError};
    use std::collections::{HashMap, HashSet};
    struct ValidParts {
        line: Vec<Event<&'static str>>,
        lookup: HashMap<FiberId, (Fiber, FiberState)>,
        purged_ids: HashSet<FiberId>,
        next_id: FiberId,
        next_event_id: EventId,
    }
    fn build_valid() -> ValidParts {
        let mut d = Line::<&'static str>::new();
        let r1 = d.create("a").unwrap();
        let _ = d.update(r1.fiber_id, "a2").unwrap();
        let r2 = d.create("b").unwrap();
        let _ = d.detach(r2.fiber_id, "b-detach").unwrap();
        ValidParts {
            line: d.line.as_slice().to_vec(),
            lookup: d.lookup.clone(),
            purged_ids: d.purged_ids.clone(),
            next_id: d.next_id,
            next_event_id: d.next_event_id,
        }
    }
    #[test]
    fn verify_invariants_accepts_public_api_built_dragline() {
        let mut d = Line::<&str>::new();
        let r1 = d.create("a").unwrap();
        let _ = d.update(r1.fiber_id, "a2").unwrap();
        let r2 = d.create("b").unwrap();
        let _ = d.detach(r2.fiber_id, "b-detach").unwrap();
        assert!(d.verify_invariants().is_ok(), "{:?}", d.verify_invariants());
    }
    #[test]
    fn verify_invariants_empty_dragline_ok() {
        let d = Line::<&str>::new();
        assert!(d.verify_invariants().is_ok());
    }
    #[test]
    fn verify_invariants_rejects_non_monotonic_event_ids() {
        let ValidParts {
            mut line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let last_idx = line.len() - 1;
        let prev_id = line[last_idx - 1].event_id();
        let dup = Event::new_unchecked(
            prev_id,
            line[last_idx].fiber_id(),
            line[last_idx].detached(),
            line[last_idx].precursor(),
            line[last_idx].precursor_hash(),
            *line[last_idx].domain_event(),
        );
        line[last_idx] = dup;
        let err = Line::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        let msg = format!("{err}");
        let expected_position = u64::try_from(last_idx).unwrap();
        assert!(
            matches!(err,
            PardosaError::FiberInvariant(FiberInvariantKind::Integrity(IntegrityKind::EventIdPositionMismatch
            { event_id, position })) if event_id == prev_id.value() && position ==
            expected_position)
                && msg.contains("expected event_id == position"),
            "got: {err}"
        );
    }
    #[test]
    fn verify_invariants_rejects_purged_id_in_lookup() {
        let ValidParts {
            line,
            lookup,
            mut purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let live_id = *lookup.keys().next().unwrap();
        purged_ids.insert(live_id);
        let err = Line::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::PurgedIdInLookup(_)
                ))
            ) && msg.contains("both purged and present in lookup"),
            "got: {err}"
        );
    }
    #[test]
    fn verify_invariants_rejects_wrong_next_event_id() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let bad_next = EventId::new(next_event_id.value() + 1);
        let err = Line::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            bad_next,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::NextEventIdMismatch { .. }
                ))
            ) && msg.contains("next_event_id"),
            "got: {err}"
        );
    }
    #[test]
    fn verify_invariants_rejects_nonzero_next_event_id_on_empty_line() {
        let err = Line::<&str>::from_parts_unchecked(
            Vec::new(),
            HashMap::new(),
            HashSet::new(),
            FiberId::new(0),
            EventId::new(1),
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::NextEventIdMismatch { .. }
                ))
            ),
            "got: {err}"
        );
    }
    #[test]
    fn verify_invariants_rejects_fiber_index_out_of_bounds() {
        let ValidParts {
            line,
            mut lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let line_len_u64 = u64::try_from(line.len()).unwrap();
        let bogus_index = Index::new(line_len_u64);
        let bogus_fiber = Fiber::new(bogus_index, 1, bogus_index).unwrap();
        lookup.insert(FiberId::new(999), (bogus_fiber, FiberState::Defined));
        let err = Line::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::Integrity(
                    IntegrityKind::FiberCurrentOutOfBounds { .. }
                ))
            ) && msg.contains(">= line.len()"),
            "got: {err}"
        );
    }
    #[test]
    fn verify_invariants_propagates_broken_precursor_chain() {
        let ValidParts {
            mut line,
            lookup,
            purged_ids,
            next_id,
            next_event_id: _,
        } = build_valid();
        let pos = u64::try_from(line.len()).unwrap();
        let bad = Event::new_unchecked(
            EventId::new(line.last().unwrap().event_id().value() + 1),
            FiberId::new(0),
            false,
            Precursor::Of(Index::new(pos + 5)),
            [0u8; 32],
            "bad",
        );
        line.push(bad);
        let new_next = EventId::new(u64::try_from(line.len()).unwrap());
        let err = Line::from_parts_unchecked(
            line,
            lookup,
            purged_ids,
            next_id,
            new_next,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "got: {err}"
        );
    }
    #[test]
    fn from_raw_parts_accepts_valid_state() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let computed = line.iter().fold(Frontier::GENESIS, |acc, e| {
            acc.roll(&pardosa_wire::to_vec(e))
        });
        let d = Line::from_raw_parts(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            computed,
        )
        .expect("valid state must round-trip through from_raw_parts");
        assert!(d.verify_invariants().is_ok());
    }
    #[test]
    fn from_raw_parts_accepts_when_supplied_matches_computed() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let computed = line.iter().fold(Frontier::GENESIS, |acc, e| {
            acc.roll(&pardosa_wire::to_vec(e))
        });
        let d = Line::from_raw_parts(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            computed,
        )
        .expect("matching supplied frontier must be accepted");
        assert_eq!(d.frontier(), computed);
    }
    #[test]
    fn from_raw_parts_rejects_frontier_mismatch() {
        let ValidParts {
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
        } = build_valid();
        let err = Line::from_raw_parts(
            line,
            lookup,
            purged_ids,
            next_id,
            next_event_id,
            false,
            Frontier::GENESIS,
        )
        .unwrap_err();
        match err {
            PardosaError::FrontierMismatch { supplied, computed } => {
                assert_eq!(supplied, Frontier::GENESIS);
                assert_ne!(computed, Frontier::GENESIS);
                assert_ne!(supplied, computed);
            }
            other => panic!("expected FrontierMismatch, got: {other:?}"),
        }
    }
}
