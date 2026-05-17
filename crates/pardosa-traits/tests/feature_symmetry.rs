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
//! `const _` symmetry probe fails to compile — surfacing the drift
//! loudly before it can ship.
//!
//! Coverage strategy
//! -----------------
//! Each `const _` probe asserts `T: sealed::Sealed + EventSafe + Decode`
//! by referencing `assert_symmetric::<T>` as a function pointer.
//! Because `EventSafe: pardosa_encoding::Encode` (supertrait, see
//! `pardosa-traits/src/lib.rs:60`), the `EventSafe` bound transitively
//! verifies `Encode` resolves — so one bound covers three of the four
//! impls. `Decode` is not a supertrait of `EventSafe`, so it appears
//! explicitly.
//!
//! Asymmetric features
//! -------------------
//! The "asymmetry test" requirement in adr-fmt-1yuy is satisfied
//! structurally: each `const _` probe lives under the same
//! `#[cfg(feature)]` predicate as the impls it asserts. With the
//! feature off, the assertion is compiled out alongside the impls —
//! no orphan check survives without its target, so absence is
//! symmetric by construction. Conversely, if `pardosa-traits` enabled
//! `uuid` without pulling in `pardosa-encoding/uuid`, the supertrait
//! `Encode for Uuid` would vanish and `pardosa-traits` itself would
//! fail to compile (the `EventSafe for Uuid` impl at lib.rs:182
//! demands `Encode for Uuid` via the supertrait clause). No trybuild
//! fixture is required because the supertrait closure already
//! produces the desired hard failure on drift.

#[cfg(any(
    feature = "uuid",
    feature = "bytes",
    feature = "arrayvec",
    feature = "jiff"
))]
use pardosa_encoding::Decode;
#[cfg(any(
    feature = "uuid",
    feature = "bytes",
    feature = "arrayvec",
    feature = "jiff"
))]
use pardosa_traits::{EventSafe, sealed};

/// Generic compile-time symmetry probe. The `const _` items below
/// instantiate this with a concrete foreign type behind the appropriate
/// feature gate. The helper itself is `#[cfg]`-gated on the disjunction
/// of feature flags that produce a caller, so it exists if and only if
/// at least one probe is compiled in.
#[cfg(any(
    feature = "uuid",
    feature = "bytes",
    feature = "arrayvec",
    feature = "jiff"
))]
fn assert_symmetric<T: sealed::Sealed + EventSafe + Decode>() {}

// Anonymous `const _` items are the idiomatic Rust shape for type-level
// assertions that must compile but produce no callable item: they force
// the compiler to resolve the bounds in `assert_symmetric::<T>` (taking
// the function as a value forces its signature to type-check) without
// introducing a `dead_code`-triggering function symbol.
#[cfg(feature = "uuid")]
const _: fn() = assert_symmetric::<uuid::Uuid>;

#[cfg(feature = "bytes")]
const _: fn() = assert_symmetric::<bytes::Bytes>;

// `ArrayVec<u8, 4>` matches the brief's exemplar bound. `u8` is a
// primitive `EventSafe + Decode`, so the bounds on `ArrayVec<T, N>:
// Sealed + EventSafe + Decode` (which require `T: EventSafe`
// / `T: Decode`) resolve cleanly.
#[cfg(feature = "arrayvec")]
const _: fn() = assert_symmetric::<arrayvec::ArrayVec<u8, 4>>;

// GEN-0043 jiff::Timestamp — parallel to the GEN-0041 v0 floor. Drift
// in either crate's `jiff` gate (e.g. dropping `EventSafe for Timestamp`
// while keeping `Encode for Timestamp`) breaks compile here.
#[cfg(feature = "jiff")]
const _: fn() = assert_symmetric::<jiff::Timestamp>;

/// Runtime no-op so `cargo test` reports a result line for this file
/// even when the symmetry assertions are purely compile-time. Keeps
/// the test surface visible in CI output.
#[test]
fn symmetry_assertions_compile() {
    // The work is done by the type checker on the `const _` probes
    // above. Reaching this line means every enabled foreign-floor pair
    // resolved its full Sealed + EventSafe + Encode + Decode quartet.
}
