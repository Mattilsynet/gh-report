//! Verifies that `Projection` is NOT dyn-compatible (object-safe).
//!
//! `cherry_pit_core::Projection` declares `type Event: DomainEvent` as an
//! associated type without a `Self: Sized` bound on the trait item, so
//! `dyn Projection` cannot be constructed. This locks the
//! generics-per-projection invariant from CHE-0048:R6 — projection
//! drivers must be statically dispatched, never erased through a
//! `Box<dyn Projection>` indirection.
//!
//! If this test ever passes-compile, the trait has become dyn-safe and
//! the static-dispatch contract is silently broken.
use cherry_pit_core::Projection;

fn _erase(_p: Box<dyn Projection>) {}

fn main() {}
