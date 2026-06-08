//! Canonical rolling-frontier fold over a persisted event line.
//!
//! The sole non-test caller of [`Frontier::roll`] outside the live
//! commit path (`dragline/commit.rs`). Encodes ADR-0004 §1:
//! frontier rolls in persisted line order, exactly once per event,
//! on canonical wire-form bytes. Consumers obtain
//! `(frontier_after, Option<Tick>)` pairs and cannot manufacture an
//! anchor without rolling through [`Frontier::roll`].
//!
//! Consumers: [`crate::dragline::recover::reconstruct_unpublished_anchors`]
//! (uses `tick`); [`crate::persist::rehydrate_unchecked`] (ignores
//! `tick`). Collapsing both onto this adapter closes F-B1. See
//! ADR-0005 for the canonical encoding contract via
//! [`pardosa_wire::to_vec`].
use crate::event::{Event, EventId};
use crate::frontier::{AnchorInterval, Frontier};
use pardosa_wire::{Encode, to_vec};
/// One tick of the anchor-interval cadence inside a [`FrontierFold`].
///
/// Produced by the fold whenever `events_since_tick` reaches the
/// configured [`AnchorInterval`]. Carries the source event's
/// [`EventId`] so the publish watermark can be advanced per-anchor
/// (ADR-0016 §D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tick {
    pub event_id: EventId,
}
/// One step of the canonical fold: the rolling frontier *after* this
/// event's canonical bytes have been chained in. `tick` is `Some` iff
/// this position completes an anchor-interval window.
#[derive(Debug)]
pub(crate) struct FoldStep {
    pub frontier_after: Frontier,
    pub tick: Option<Tick>,
}
/// Iterator-shaped fold that lock-steps the rolling frontier with the
/// event line. Construct with [`FrontierFold::new`]; each
/// [`Iterator::next`] call rolls exactly one event's canonical bytes
/// into the frontier and yields the resulting [`FoldStep`].
///
/// The internal `frontier` field is private — consumers cannot inject
/// arbitrary bytes into the roll. The only path from an
/// `&Event<T>` to a yielded `frontier_after` is through this fold,
/// which is the ADR-0004 §1 invariant in type-system form.
pub(crate) struct FrontierFold<'a, T> {
    line: std::slice::Iter<'a, Event<T>>,
    interval: u64,
    frontier: Frontier,
    events_since_tick: u64,
}
impl<'a, T> FrontierFold<'a, T> {
    /// Begin a fold from [`Frontier::GENESIS`] over `line`.
    ///
    /// `interval` is the [`AnchorInterval`] in effect for the line;
    /// the fold emits a `Tick` whenever `events_since_tick` reaches
    /// `interval.get()`. Callers that do not need ticks (e.g.
    /// [`crate::persist::rehydrate_unchecked`]) pass any non-zero
    /// interval and ignore `step.tick`.
    #[inline]
    pub(crate) fn new(line: &'a [Event<T>], interval: AnchorInterval) -> Self {
        Self {
            line: line.iter(),
            interval: interval.get(),
            frontier: Frontier::GENESIS,
            events_since_tick: 0,
        }
    }
    /// Frontier value after the most recently yielded step (or
    /// [`Frontier::GENESIS`] if [`Iterator::next`] has not been called
    /// or the line was empty). Test-only — production callers
    /// (`reconstruct_unpublished_anchors`) consume `frontier_after`
    /// per step; the reader path no longer rebuilds via `FrontierFold`
    /// (ADR-0020).
    #[cfg(test)]
    pub(crate) fn frontier(&self) -> Frontier {
        self.frontier
    }
}
impl<T> Iterator for FrontierFold<'_, T>
where
    T: Encode,
{
    type Item = FoldStep;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let event = self.line.next()?;
        let bytes = to_vec(event);
        self.frontier = self.frontier.roll(&bytes);
        self.events_since_tick = self.events_since_tick.saturating_add(1);
        let tick = if self.events_since_tick >= self.interval {
            self.events_since_tick = 0;
            Some(Tick {
                event_id: event.event_id(),
            })
        } else {
            None
        };
        Some(FoldStep {
            frontier_after: self.frontier,
            tick,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::super::state::Line;
    use super::*;
    /// ADR-0004 byte-identity: the fold over a line's events
    /// yields the same final frontier the live writer rolled into the
    /// line at commit time. This pins the invariant the two batch
    /// consumers (`reconstruct_unpublished_anchors`,
    /// `rehydrate_unchecked`) both rely on.
    #[test]
    fn fold_matches_live_writer_frontier() {
        let mut d: Line<u64> = Line::new();
        for i in 0..7u64 {
            let _ = d.create(i).expect("create");
        }
        let live_frontier = d.frontier();
        let mut fold = FrontierFold::new(d.read_line(), AnchorInterval::ONE);
        while fold.next().is_some() {}
        assert_eq!(
            fold.frontier(),
            live_frontier,
            "fold frontier must match live-writer frontier (ADR-0004 §1)"
        );
    }
    #[test]
    fn fold_empty_line_yields_genesis() {
        let d: Line<u64> = Line::new();
        let mut fold = FrontierFold::new(d.read_line(), AnchorInterval::ONE);
        assert!(fold.next().is_none());
        assert_eq!(fold.frontier(), Frontier::GENESIS);
    }
    #[test]
    fn fold_emits_tick_at_interval_boundary() {
        let mut d: Line<u64> = Line::new();
        for i in 0..6u64 {
            let _ = d.create(i).expect("create");
        }
        let interval = AnchorInterval::try_new(3).expect("3 is non-zero");
        let ticks: Vec<EventId> = FrontierFold::new(d.read_line(), interval)
            .filter_map(|s| s.tick.map(|t| t.event_id))
            .collect();
        assert_eq!(ticks.len(), 2, "6 events / interval 3 == 2 ticks");
        assert_eq!(ticks[0].value(), 2);
        assert_eq!(ticks[1].value(), 5);
    }
    #[test]
    fn fold_frontier_after_is_monotonic_per_step() {
        let mut d: Line<u64> = Line::new();
        for i in 0..4u64 {
            let _ = d.create(i).expect("create");
        }
        let frontiers: Vec<Frontier> = FrontierFold::new(d.read_line(), AnchorInterval::ONE)
            .map(|s| s.frontier_after)
            .collect();
        assert_eq!(frontiers.len(), 4);
        for w in frontiers.windows(2) {
            assert_ne!(w[0], w[1]);
        }
    }
}
