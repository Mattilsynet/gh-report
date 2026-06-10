//! Verifies that `EventStore` is NOT dyn-compatible (object-safe).
//!
//! `cherry_pit_core::EventStore` declares `type Event: DomainEvent` and
//! returns `impl Future` from its methods, both of which preclude
//! dyn-compatibility. This locks the single-event-type-per-store
//! invariant from CHE-0005:R1 — every concrete store is monomorphic
//! over exactly one `DomainEvent` impl, never erased through a
//! `Box<dyn EventStore>` indirection.
//!
//! `cherry-pit-app` carries its own backstop (in addition to the
//! `cherry-pit-projection` mirror at `eae00d3`) because `App` holds
//! the store as a `S: EventStore` generic parameter (`app.rs`); a
//! future refactor that accidentally erased the store would silently
//! break CHE-0005:R1 here too. Pattern mirrors WU-3 SM-3.2 (`eae00d3`)
//! and WU-4 SM-4.2 (`6472df4`).
//!
//! If this test ever passes-compile, the trait has become dyn-safe and
//! the one-event-type-per-store contract is silently broken.
use cherry_pit_core::EventStore;

fn _erase(_s: Box<dyn EventStore>) {}

fn main() {}
