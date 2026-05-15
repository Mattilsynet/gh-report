//! Verifies that `ProjectionSource` is NOT dyn-compatible (object-safe).
//!
//! `cherry_pit_web::ProjectionSource` is sealed against `dyn` use via a
//! private `fn _seal(&self) where Self: Sized {}` method (see
//! `crates/cherry-pit-web/src/projection/port.rs`). That `Self: Sized`
//! bound makes the trait not dyn-compatible, so `Box<dyn ProjectionSource>`
//! cannot be constructed.
//!
//! This locks CHE-0049:R12 — projection sources MUST be consumed as a
//! generic type parameter `P: ProjectionSource` on `ProjectionState<P>`
//! / `build_projection_router<P>`, never as a trait object. The seal
//! upgrades that contract from CONVENTION to COVERED.
//!
//! If this test ever passes-compile, the seal has been removed and the
//! static-dispatch contract is silently broken. Pattern mirrors WU-1
//! SM-1.2 and WU-3 SM-3.2 (cherry-pit-projection compile_fail tests).
use cherry_pit_web::ProjectionSource;

fn _erase(_p: Box<dyn ProjectionSource>) {}

fn main() {}
