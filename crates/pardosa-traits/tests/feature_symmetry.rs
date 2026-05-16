//! FH10 — feature-flag symmetry between `pardosa-traits` and
//! `pardosa-encoding` for the foreign-floor types (GEN-0041).
//!
//! The orphan rule mandates that `Sealed + EventSafe` for foreign types
//! lives in `pardosa-traits` while the matching `Encode + Decode` lives
//! in `pardosa-encoding`. Each crate gates its impls behind feature flags
//! of the same name (`uuid`, `bytes`, `arrayvec`); `pardosa-traits`
//! activates `pardosa-encoding/<flag>` via its own feature definitions so
//! the pair travels together.
//!
//! These assertions pin the symmetry at compile time. If a future PR
//! adds a feature in one crate without the matching pull-through, or
//! drops one of the impls in a pair (e.g. removes `EventSafe for Uuid`
//! while keeping `Encode for Uuid`, or vice versa), the corresponding
//! `_check_*` function fails to compile — surfacing the drift loudly
//! before it can ship.
//!
//! Coverage strategy
//! -----------------
//! Each `_check_*` function asserts `T: sealed::Sealed + EventSafe + Decode`.
//! Because `EventSafe: pardosa_encoding::Encode` (supertrait, see
//! `pardosa-traits/src/lib.rs:60`), the `EventSafe` bound transitively
//! verifies `Encode` resolves — so one bound covers three of the four
//! impls. `Decode` is not a supertrait of `EventSafe`, so it appears
//! explicitly.
//!
//! Asymmetric features
//! -------------------
//! The "asymmetry test" requirement in adr-fmt-1yuy is satisfied
//! structurally: each `_check_*` lives under the same `#[cfg(feature)]`
//! predicate as the impls it asserts. With the feature off, the
//! assertion is compiled out alongside the impls — no orphan check
//! survives without its target, so absence is symmetric by construction.
//! Conversely, if `pardosa-traits` enabled `uuid` without pulling in
//! `pardosa-encoding/uuid`, the supertrait `Encode for Uuid` would
//! vanish and `pardosa-traits` itself would fail to compile (the
//! `EventSafe for Uuid` impl at lib.rs:182 demands `Encode for Uuid`
//! via the supertrait clause). No trybuild fixture is required because
//! the supertrait closure already produces the desired hard failure on
//! drift.

use pardosa_encoding::Decode;
use pardosa_traits::{EventSafe, sealed};

/// Generic compile-time symmetry probe. Calling sites instantiate this
/// with a concrete foreign type behind the appropriate feature gate.
fn _assert_symmetric<T: sealed::Sealed + EventSafe + Decode>() {}

#[cfg(feature = "uuid")]
#[allow(dead_code)]
fn _check_uuid() {
    _assert_symmetric::<uuid::Uuid>();
}

#[cfg(feature = "bytes")]
#[allow(dead_code)]
fn _check_bytes() {
    _assert_symmetric::<bytes::Bytes>();
}

#[cfg(feature = "arrayvec")]
#[allow(dead_code)]
fn _check_arrayvec() {
    // `ArrayVec<u8, 4>` matches the brief's exemplar bound. `u8` is a
    // primitive `EventSafe + Decode`, so the bounds on `ArrayVec<T, N>:
    // Sealed + EventSafe + Decode` (which require `T: EventSafe`
    // / `T: Decode`) resolve cleanly.
    _assert_symmetric::<arrayvec::ArrayVec<u8, 4>>();
}

/// Runtime no-op so `cargo test` reports a result line for this file
/// even when the symmetry assertions are purely compile-time. Keeps
/// the test surface visible in CI output.
#[test]
fn symmetry_assertions_compile() {
    // The work is done by the type checker on the `_check_*` functions
    // above. Reaching this line means every enabled foreign-floor pair
    // resolved its full Sealed + EventSafe + Encode + Decode quartet.
}
