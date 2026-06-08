use super::state::{AppendResult, Line};
use crate::error::PardosaError;
use crate::event::{Event, EventId, FiberId, Index};
use crate::fiber::Fiber;
use crate::fiber_state::FiberState;
use pardosa_wire::{Encode, to_vec};
pub(super) struct PreparedCommit<T> {
    pub(super) event: Event<T>,
    pub(super) event_id: EventId,
    pub(super) fiber_id: FiberId,
    pub(super) lookup_op: LookupOp,
    pub(super) next_id_advance: Option<FiberId>,
}
pub(super) enum LookupOp {
    Insert {
        fiber: Fiber,
        state: FiberState,
    },
    AdvanceFiber {
        new_current: Index,
        new_state: Option<FiberState>,
    },
}
impl<T> Line<T> {
    pub(super) fn reject_if_migrating(&self) -> Result<(), PardosaError> {
        if self.migrating {
            Err(PardosaError::MigrationInProgress)
        } else {
            Ok(())
        }
    }
    pub(super) fn peek_event_id(&self) -> Result<EventId, PardosaError> {
        if self.next_event_id.value() == u64::MAX {
            Err(PardosaError::EventIdOverflow)
        } else {
            Ok(self.next_event_id)
        }
    }
    pub(super) fn next_index(&self) -> Result<Index, PardosaError> {
        let len = u64::try_from(self.line.len()).map_err(|_| PardosaError::IndexOverflow)?;
        if len == u64::MAX {
            return Err(PardosaError::IndexOverflow);
        }
        Ok(Index::from_decoded(len))
    }
    pub(super) fn commit_atomic<F>(&mut self, prepare: F) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
        F: FnOnce(&Self) -> Result<PreparedCommit<T>, PardosaError>,
    {
        self.reject_if_migrating()?;
        let prepared = prepare(self)?;
        self.apply_prepared(prepared)
    }
    fn apply_prepared(&mut self, p: PreparedCommit<T>) -> Result<AppendResult, PardosaError>
    where
        T: Encode,
    {
        let would_tick = self.events_since_tick.saturating_add(1) >= self.anchor_interval.get();
        if would_tick
            && let Some(buf) = self.pending_anchors.as_ref()
            && buf.len() >= self.anchor_buffer_cap
        {
            return Err(PardosaError::AnchorBufferOverflow {
                cap: self.anchor_buffer_cap,
            });
        }
        let PreparedCommit {
            event,
            event_id,
            fiber_id,
            lookup_op,
            next_id_advance,
        } = p;
        let event_bytes = to_vec(&event);
        self.frontier = self.frontier.roll(&event_bytes);
        self.line.append_validated(event, event_id)?;
        match lookup_op {
            LookupOp::Insert { fiber, state } => {
                self.lookup.insert(fiber_id, (fiber, state));
            }
            LookupOp::AdvanceFiber {
                new_current,
                new_state,
            } => {
                let (fiber, state) = self
                    .lookup
                    .get_mut(&fiber_id)
                    .expect("prepare verified fiber presence");
                fiber.advance_unchecked(new_current);
                if let Some(ns) = new_state {
                    *state = ns;
                }
            }
        }
        if let Some(d) = next_id_advance {
            self.next_id = d;
        }
        self.next_event_id = event_id.checked_next()?;
        self.events_since_tick += 1;
        if would_tick {
            self.events_since_tick = 0;
            if let Some(buf) = self.pending_anchors.as_mut() {
                let subject = format!("pardosa.{}.frontier", self.stream_name);
                buf.push((subject, *self.frontier.as_bytes()));
            }
        }
        Ok(AppendResult { fiber_id, event_id })
    }
}
