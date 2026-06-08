use super::state::Line;
#[cfg(test)]
use crate::event::EventId;
use crate::event::FiberId;
use crate::fiber_state::FiberState;
use crate::frontier::AnchorInterval;
impl<T> Line<T> {
    #[must_use]
    #[cfg(test)]
    pub fn next_event_id(&self) -> EventId {
        self.next_event_id
    }
    #[must_use]
    #[cfg(test)]
    pub fn next_fiber_id(&self) -> FiberId {
        self.next_id
    }
    #[must_use]
    #[cfg(test)]
    pub fn line_len(&self) -> usize {
        self.line.len()
    }
    #[must_use]
    pub fn fiber_state(&self, fiber_id: FiberId) -> FiberState {
        if let Some((_, state)) = self.lookup.get(&fiber_id) {
            *state
        } else if self.purged_ids.contains(&fiber_id) {
            FiberState::Purged
        } else {
            FiberState::Undefined
        }
    }
    /// In-crate accessor used by [`super::recover::reconstruct_unpublished_anchors`]
    /// to read the configured `anchor_interval`. Hidden from the public
    /// surface because the recovery pipeline is a [`crate::dragline::Dragline`]
    /// internal — readers receive a [`super::view::DraglineView`] which
    /// does not expose this method (ADR-0016 §D2).
    pub(crate) fn anchor_interval_for_recover(&self) -> AnchorInterval {
        self.anchor_interval
    }
    /// In-crate accessor for the configured stream name; same scope and
    /// reasoning as [`Self::anchor_interval_for_recover`].
    pub(crate) fn stream_name_for_recover(&self) -> &str {
        &self.stream_name
    }
    /// Test-only hook to set `stream_name` and `anchor_interval` on an
    /// already-rehydrated `Line` so the in-crate recover tests can
    /// mirror the configuration the owning `Dragline` runtime will apply at
    /// restart. Production callers route through
    /// [`crate::dragline::Dragline`] constructors, which set both at
    /// construction time.
    #[cfg(test)]
    pub(crate) fn set_recover_config_for_test(
        &mut self,
        stream_name: String,
        anchor_interval: u64,
    ) {
        self.stream_name = stream_name;
        self.anchor_interval = AnchorInterval::new_or_one(anchor_interval);
    }
    /// In-crate setter for `stream_name` + `anchor_interval` used by
    /// [`crate::dragline::Dragline`] restart constructors to configure
    /// a rehydrated line before reconstruction.
    ///
    /// Not public: configuration belongs on the runtime-owned writer
    /// surface, not the in-memory line (a reader may borrow via
    /// `DraglineView`). Also clears any `pending_anchors` buffer —
    /// the durable publish path (ADR-0016 §D6) reconstructs anchors
    /// from the persisted line + watermark, so the live buffer is
    /// unused. Leaving it `Some(...)` caused false
    /// [`AnchorBufferOverflow`](crate::PardosaError::AnchorBufferOverflow)
    /// under sustained durable publishing (mission
    /// `p0-storage-qf9h-20260525`).
    pub(crate) fn configure_recover(&mut self, stream_name: String, anchor_interval: u64) {
        self.stream_name = stream_name;
        self.anchor_interval = AnchorInterval::new_or_one(anchor_interval);
        self.pending_anchors = None;
    }
}
