//! Port trait: read-side projection source consumed by the web adapter.
//!
//! Phase 2 deliberately keeps the surface minimal — three methods, no
//! associated types, no `serde` decode-bound bleed anywhere on the trait
//! or its parameters (closes A3, honours CHE-0014 R2).
//!
//! Per CHE-0049 R12 + CHE-0005 R1, downstream code binds this trait as a
//! generic parameter `P` on [`super::state::ProjectionState`] /
//! [`super::build_projection_router`] — never as `Box<dyn …>` /
//! `Arc<dyn …>`. Trait-object usage is a compile-time error by design.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;

use super::state::{PageEntry, PageUpdate};

/// Read-side projection adapter that the web layer queries for snapshots
/// and subscribes to for deltas.
///
/// Phase 2 defines the *shape only*; Phase 3 wires real handlers + WS.
/// Implementations are owned by consumers (typically wrapping a
/// `cherry_pit_projection` driver). No method here returns an error —
/// readiness is exposed via [`is_ready`](Self::is_ready) per CHE-0049
/// R11 (HTTP-snapshot-then-WS-deltas reconnect protocol).
pub trait ProjectionSource: Send + Sync + 'static {
    /// Return the current durable snapshot, if one is available.
    ///
    /// Returns `None` before [`is_ready`](Self::is_ready) flips to `true`.
    /// The snapshot is shared via `Arc` to avoid copying the page map per
    /// request (Phase 3 detail; Phase 2 honours the signature only).
    fn snapshot(&self) -> Option<Arc<HashMap<String, PageEntry>>>;

    /// Subscribe to the delta stream that follows the latest snapshot.
    ///
    /// Phase 2 honours the signature only. Phase 3 wires this into the
    /// `/ws` upgrade per CHE-0049 R11+R13.
    fn subscribe(&self) -> broadcast::Receiver<PageUpdate>;

    /// Whether the adapter has caught up to a usable snapshot.
    fn is_ready(&self) -> bool;

    // CHE-0049:R12 + mission wu4-web-closure SM-4.2:
    // Seal the trait against `dyn` use. A method with a generic type
    // parameter is excluded from a vtable, which makes the trait not
    // dyn-compatible (object-safe). The compiler emits E0038 for any
    // `Box<dyn ProjectionSource>` / `Arc<dyn ProjectionSource>` /
    // `&dyn ProjectionSource` construction. The intended consumption
    // pattern is the generic parameter `P: ProjectionSource` on
    // `ProjectionState<P>` / `build_projection_router<P>` per CHE-0005:R1
    // — this seal upgrades that contract from CONVENTION to COVERED,
    // locked by the trybuild compile_fail test under `tests/compile_fail/`.
    //
    // Implementors provide the empty default; the method is doc-hidden
    // and never called from public APIs.
    #[doc(hidden)]
    fn __seal_no_dyn<__Seal>(&self, _seal: __Seal) {}
}
