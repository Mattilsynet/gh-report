//! Two-phase commit machinery shared by all `Dragline` writers (FH5).
//!
//! Every `Dragline` writer (`create`, `create_reuse`, `update`, `detach`,
//! `rescue`) discharges its fallibility in a `prepare` closure that returns
//! a [`PreparedCommit`]; [`Dragline::commit_atomic`] then drives the
//! infallible mutation phase via `apply_prepared`. Partial commits are
//! therefore unrepresentable: the only remaining fallible step inside
//! `apply_prepared` is the `Linevec::append_validated` call, whose
//! invariants were already pre-validated upstream — an Err there is a
//! same-writer bug, not user error.

use super::all::{AppendResult, Dragline};
use crate::error::PardosaError;
use crate::event::{DomainId, Event, Index};
use crate::fiber::Fiber;
use crate::fiber_state::FiberState;

/// Description of a prepared, infallible mutation produced by the
/// `prepare` phase of `Dragline::commit_atomic` (FH5). Every field
/// here has had its fallibility discharged in `prepare`; `apply` only
/// performs operations whose invariants were checked upstream.
pub(super) struct PreparedCommit<T> {
    pub(super) event: Event<T>,
    pub(super) event_id: u64,
    pub(super) index: Index,
    pub(super) domain_id: DomainId,
    pub(super) lookup_op: LookupOp,
    /// Some when this writer advances the domain-id counter (create).
    pub(super) next_id_advance: Option<DomainId>,
    /// Some when this writer clears a purged-id reservation (`create_reuse`).
    pub(super) purged_remove: Option<DomainId>,
}

/// Shape of the lookup-table mutation a writer performs. Each variant
/// corresponds to one of the legal write patterns; encoding them as an
/// enum prevents writers from mixing patterns or forgetting steps.
pub(super) enum LookupOp {
    /// Fresh fiber under a new domain id. Used by `create` and
    /// `create_reuse`.
    Insert { fiber: Fiber, state: FiberState },
    /// Advance an existing fiber's `current` index. `new_state` is Some
    /// when the writer also transitions the fiber state (detach,
    /// rescue Detached → Defined); None for plain `update`.
    ///
    /// `new_current` MUST have been validated via `Fiber::check_advance`
    /// in prepare; `apply` invokes `Fiber::advance_unchecked`.
    AdvanceFiber {
        new_current: Index,
        new_state: Option<FiberState>,
    },
    /// Replace the existing fiber wholesale (rescue Locked → Defined,
    /// where history is destroyed and a fresh fiber starts).
    ReplaceFiber { fiber: Fiber, new_state: FiberState },
}

impl<T> Dragline<T> {
    // ── Internal helpers ──────────────────────────────────────────────

    pub(super) fn reject_if_migrating(&self) -> Result<(), PardosaError> {
        if self.migrating {
            Err(PardosaError::MigrationInProgress)
        } else {
            Ok(())
        }
    }

    pub(super) fn peek_event_id(&self) -> Result<u64, PardosaError> {
        if self.next_event_id == u64::MAX {
            Err(PardosaError::EventIdOverflow)
        } else {
            Ok(self.next_event_id)
        }
    }

    pub(super) fn next_index(&self) -> Result<Index, PardosaError> {
        let len = u64::try_from(self.line.len()).map_err(|_| PardosaError::IndexOverflow)?;
        // u64::MAX is reserved for Index::NONE. Reject if next index would be the sentinel.
        if len == u64::MAX {
            return Err(PardosaError::IndexOverflow);
        }
        Ok(Index::new(len))
    }

    // ── Atomic-commit helper (FH5) ────────────────────────────────────
    //
    // The four/five Dragline writers (create, create_reuse, update,
    // detach, rescue) all share a pre-validate-then-mutate shape. To
    // make partial commits unrepresentable, every fallible computation
    // is hoisted into a `prepare` closure that returns a `PreparedCommit`
    // describing the mutations to perform. `commit_atomic` then drives
    // the mutations through `apply_prepared`, which is infallible: the
    // line append is the last operation that can fail (it is gated by
    // `Linevec::append_validated`'s pre-validation), and every step
    // after it consists of operations whose fallibility was already
    // discharged in `prepare` (including `Fiber::advance`, lifted into
    // `Fiber::check_advance` here and replayed via
    // `Fiber::advance_unchecked` in apply).

    pub(super) fn commit_atomic<F>(&mut self, prepare: F) -> Result<AppendResult, PardosaError>
    where
        F: FnOnce(&Self) -> Result<PreparedCommit<T>, PardosaError>,
    {
        self.reject_if_migrating()?;
        let prepared = prepare(self)?;
        self.apply_prepared(prepared)
    }

    /// Infallible mutation phase. Every error path was discharged in
    /// `prepare`; if `line.append_validated` returns Err here it
    /// indicates a same-writer bug — the prepare phase computed a
    /// candidate that violates Linevec's invariants. Surface loudly.
    fn apply_prepared(&mut self, p: PreparedCommit<T>) -> Result<AppendResult, PardosaError> {
        let PreparedCommit {
            event,
            event_id,
            index,
            domain_id,
            lookup_op,
            next_id_advance,
            purged_remove,
        } = p;

        // Single remaining fallible step. Anything but Ok here means
        // prepare missed a validation — bug, not user error.
        self.line.append_validated(event, event_id)?;

        match lookup_op {
            LookupOp::Insert { fiber, state } => {
                self.lookup.insert(domain_id, (fiber, state));
            }
            LookupOp::AdvanceFiber {
                new_current,
                new_state,
            } => {
                let (fiber, state) = self
                    .lookup
                    .get_mut(&domain_id)
                    .expect("prepare verified fiber presence");
                fiber.advance_unchecked(new_current);
                if let Some(ns) = new_state {
                    *state = ns;
                }
            }
            LookupOp::ReplaceFiber { fiber, new_state } => {
                let (slot_fiber, slot_state) = self
                    .lookup
                    .get_mut(&domain_id)
                    .expect("prepare verified fiber presence");
                *slot_fiber = fiber;
                *slot_state = new_state;
            }
        }

        if let Some(d) = purged_remove {
            self.purged_ids.remove(&d);
        }
        if let Some(d) = next_id_advance {
            self.next_id = d;
        }
        self.next_event_id = event_id + 1;

        Ok(AppendResult {
            domain_id,
            event_id,
            index,
        })
    }
}
