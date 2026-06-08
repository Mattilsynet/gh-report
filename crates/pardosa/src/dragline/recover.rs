//! Crash-recovery helpers for the writer-side publish path
//! (ADR-0016 §§D5–D8).
//!
//! Per ADR-0004, every anchor is the rolling BLAKE3 frontier at an
//! anchor-interval tick, folded in persisted line order over
//! canonical wire-form bytes ([`pardosa_wire::to_vec`]). The `.pgno`
//! line is the sole input, so anchors a live writer would have
//! rolled are reproducible byte-identically from any rehydrated
//! [`Line`] (ADR-0016 §D7).
//!
//! Exposes [`reconstruct_unpublished_anchors`] used by
//! `Dragline::sync_data`.
use super::fold::FrontierFold;
use super::state::Line;
use crate::event::EventId;
use pardosa_wire::Encode;
/// One reconstructed anchor with the source `event_id` of the
/// commit at which the anchor-interval tick fired. The triple is
/// the canonical drain unit for [`crate::dragline::Dragline::sync_data_with_source`]
/// on the recovery path: `event_id` advances the publish watermark
/// per-anchor, `subject` + `payload` ship to the
/// [`crate::frontier::FrontierPublisher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconstructedAnchor {
    pub event_id: EventId,
    pub subject: String,
    pub payload: [u8; 32],
}
/// Re-fold the rolling frontier over `line`'s events and emit
/// every anchor whose source event has `event_id > watermark`.
///
/// Uses the line's `stream_name` and `anchor_interval`, so
/// `(subject, payload)` tuples are byte-identical to a live
/// writer's at the same cadence. `watermark = None` emits every
/// anchor; `Some(id)` skips the prefix `event_id <= id` at emit
/// time while still folding them (ADR-0004 binding). Anchors are
/// returned in commit order; empty = watermark covers everything
/// reconstructible (ADR-0016 §D9 test 3).
pub(crate) fn reconstruct_unpublished_anchors<T>(
    line: &Line<T>,
    watermark: Option<EventId>,
) -> Vec<ReconstructedAnchor>
where
    T: Encode,
{
    let interval = line.anchor_interval_for_recover();
    let stream_name = line.stream_name_for_recover();
    let events = line.read_line();
    if events.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<ReconstructedAnchor> = Vec::new();
    let subject = format!("pardosa.{stream_name}.frontier");
    let threshold = watermark.map(super::super::event::EventId::value);
    for step in FrontierFold::new(events, interval) {
        let Some(tick) = step.tick else { continue };
        let above_watermark = match threshold {
            None => true,
            Some(w) => tick.event_id.value() > w,
        };
        if above_watermark {
            out.push(ReconstructedAnchor {
                event_id: tick.event_id,
                subject: subject.clone(),
                payload: *step.frontier_after.as_bytes(),
            });
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
    use crate::persist::{persist_with_source, rehydrate_unchecked as rehydrate};
    use std::io::Cursor;
    /// Build a dragline configured for anchor emission, commit `n`
    /// events through it, persist to a fresh sink, and return both
    /// the bytes and the anchor stream the dragline would have
    /// dispatched in-process. The anchor list is the byte-identity
    /// oracle for the recover path: ADR-0016 §D7 requires
    /// [`reconstruct_unpublished_anchors`] to emit the same
    /// `(subject, payload)` pairs after rehydrate.
    fn live_fixture(stream: &str, n: u64) -> (Vec<u8>, Vec<(String, [u8; 32])>) {
        let mut dragline: Line<u64> =
            Line::with_anchor_config(stream.to_owned(), 1, DEFAULT_ANCHOR_BUFFER_CAP);
        for i in 0..n {
            let _ = dragline.create(i).expect("commit");
        }
        let live_anchors = dragline.drain_pending_anchors();
        let mut sink = Cursor::new(Vec::new());
        persist_with_source(&dragline, &mut sink, None).expect("persist");
        (sink.into_inner(), live_anchors)
    }
    #[test]
    fn reconstruct_matches_live_oracle_with_no_watermark() {
        let (bytes, live_anchors) = live_fixture("recover", 5);
        let mut rehydrated: Line<u64> = rehydrate(Cursor::new(bytes)).expect("rehydrate");
        rehydrated.set_recover_config_for_test("recover".to_owned(), 1);
        let recovered = reconstruct_unpublished_anchors(&rehydrated, None);
        let recovered_pairs: Vec<(String, [u8; 32])> = recovered
            .into_iter()
            .map(|a| (a.subject, a.payload))
            .collect();
        assert_eq!(
            recovered_pairs, live_anchors,
            "byte-identical anchor reconstruction (ADR-0004 / ADR-0016 §D7)"
        );
    }
    #[test]
    fn watermark_covers_entire_line_emits_nothing() {
        let (bytes, _) = live_fixture("covered", 3);
        let mut d: Line<u64> = rehydrate(Cursor::new(bytes)).expect("rehydrate");
        d.set_recover_config_for_test("covered".to_owned(), 1);
        let last = EventId::new(2);
        let recovered = reconstruct_unpublished_anchors(&d, Some(last));
        assert!(
            recovered.is_empty(),
            "watermark covering full line emits zero anchors (ADR-0016 §D9 test 3)"
        );
    }
    #[test]
    fn watermark_below_last_anchor_emits_tail_only() {
        let (bytes, _) = live_fixture("tail", 5);
        let mut d: Line<u64> = rehydrate(Cursor::new(bytes)).expect("rehydrate");
        d.set_recover_config_for_test("tail".to_owned(), 1);
        let recovered = reconstruct_unpublished_anchors(&d, Some(EventId::new(2)));
        assert_eq!(recovered.len(), 2, "tail-only emission above watermark");
        for a in &recovered {
            assert_eq!(a.subject, "pardosa.tail.frontier");
        }
        assert_eq!(recovered[0].event_id.value(), 3);
        assert_eq!(recovered[1].event_id.value(), 4);
    }
    #[test]
    fn empty_dragline_emits_nothing() {
        let d: Line<u64> = Line::new();
        let recovered = reconstruct_unpublished_anchors(&d, None);
        assert!(recovered.is_empty());
    }
}
