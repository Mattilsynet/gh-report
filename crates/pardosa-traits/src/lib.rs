//! Sealed trait substrate for pardosa events (GEN-0036).
//!
//! Hosts the root marker trait [`EventSafe`] and the private sealing module
//! [`sealed`]. The trait stack is `GenomeOrd: GenomeSafe: EventSafe: Sealed`;
//! `Sealed` is the gating supertrait. External crates cannot construct a
//! `Sealed` impl, so they cannot impl `EventSafe` (or anything above it) for
//! their own types. The only blessed path is `#[derive(GenomeSafe)]`, which
//! emits `Sealed + EventSafe + GenomeSafe` atomically.
//!
//! `EventSafe` carries the [`pardosa_encoding::Encode`] supertrait, closing the
//! F2 deferral from A2.1 (see GEN-0036 Context / GEN-0037). Every sealed type
//! also has an `Encode` impl in `pardosa-encoding`; `Decode` blanket fill for
//! the same set is sub-mission B.
//!
//! # Why std-aware substrate
//!
//! The trusted-blanket pattern requires that impls of a foreign trait for
//! foreign types live in the trait's defining crate (Rust orphan rule E0117/
//! E0210). Sealed + EventSafe blankets for std types (`Box<T>`, `Vec<T>`,
//! `Arc<T>`, `BTreeMap<K, V>`, primitives, tuples, …) therefore live here.
//! `pardosa-traits` keeps zero external dependencies — std types only.

#![forbid(unsafe_code)]

use core::marker::PhantomData;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Sealing module. The `Sealed` trait is `pub` so it can appear in supertrait
/// bounds, but its only impls live in trusted crates that depend on
/// `pardosa-traits` plus the trusted blankets below. External crates cannot
/// impl `Sealed` for their own types, so they cannot impl `EventSafe` either.
pub mod sealed {
    /// Sealing supertrait. Implementing this is the gate that proves a type
    /// has been blessed by the derive macro (or a trusted std blanket impl).
    pub trait Sealed {}
}

/// Root marker trait of the pardosa event-type stack.
///
/// Implementations are restricted by the [`sealed::Sealed`] supertrait. Only
/// trusted crates in the pardosa workspace (and `#[derive(GenomeSafe)]`-blessed
/// user types) can satisfy the bound.
///
/// Every `EventSafe` type also implements [`pardosa_encoding::Encode`] — the
/// F2 supertrait bound deferred in A2.1 lands here, atomically with the
/// matching `Encode` blanket fill in `pardosa-encoding` (GEN-0037).
pub trait EventSafe: pardosa_encoding::Encode + sealed::Sealed {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — primitives
// ---------------------------------------------------------------------------
//
// Sealed + EventSafe coverage for every type with a hand-written GenomeSafe
// impl in pardosa-genome. Co-located here (not in pardosa-genome) because the
// orphan rule forbids impl'ing foreign traits for foreign types from a
// non-defining crate.

macro_rules! seal_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl sealed::Sealed for $ty {}
            impl EventSafe for $ty {}
        )+
    };
}

seal_primitive!(
    bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, char
);

impl sealed::Sealed for () {}
impl EventSafe for () {}

impl sealed::Sealed for str {}
impl EventSafe for str {}

impl sealed::Sealed for String {}
impl EventSafe for String {}

impl sealed::Sealed for &str {}
impl EventSafe for &str {}

impl sealed::Sealed for &[u8] {}
impl EventSafe for &[u8] {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — std containers and wrappers
// ---------------------------------------------------------------------------
//
// Bounds intentionally use `T: EventSafe` (not `T: GenomeSafe`) — pardosa-traits
// cannot know GenomeSafe. The wider EventSafe bound is safe: every GenomeSafe
// type is by construction EventSafe (supertrait), and these blankets only
// need the sealing chain to reach down to leaf types.

impl<T: EventSafe> sealed::Sealed for Option<T> {}
impl<T: EventSafe> EventSafe for Option<T> {}

impl<T: EventSafe> sealed::Sealed for Vec<T> {}
impl<T: EventSafe> EventSafe for Vec<T> {}

impl<T: EventSafe> sealed::Sealed for Box<T> {}
impl<T: EventSafe> EventSafe for Box<T> {}

impl<K: EventSafe + Ord, V: EventSafe> sealed::Sealed for BTreeMap<K, V> {}
impl<K: EventSafe + Ord, V: EventSafe> EventSafe for BTreeMap<K, V> {}

impl<T: EventSafe + Ord> sealed::Sealed for BTreeSet<T> {}
impl<T: EventSafe + Ord> EventSafe for BTreeSet<T> {}

impl<T: EventSafe> sealed::Sealed for Arc<T> {}
impl<T: EventSafe> EventSafe for Arc<T> {}

impl<T: EventSafe + ToOwned + ?Sized> sealed::Sealed for Cow<'_, T> {}
impl<T: EventSafe + ToOwned + ?Sized> EventSafe for Cow<'_, T> {}

impl<T: EventSafe + ?Sized> sealed::Sealed for PhantomData<T> {}
impl<T: EventSafe + ?Sized> EventSafe for PhantomData<T> {}

impl<T: EventSafe, const N: usize> sealed::Sealed for [T; N] {}
impl<T: EventSafe, const N: usize> EventSafe for [T; N] {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — tuples (1..=16, matching serde and pardosa-genome)
// ---------------------------------------------------------------------------

macro_rules! seal_tuple {
    ($($T:ident),+) => {
        impl<$($T: EventSafe),+> sealed::Sealed for ($($T,)+) {}
        impl<$($T: EventSafe),+> EventSafe for ($($T,)+) {}
    };
}

seal_tuple!(T0);
seal_tuple!(T0, T1);
seal_tuple!(T0, T1, T2);
seal_tuple!(T0, T1, T2, T3);
seal_tuple!(T0, T1, T2, T3, T4);
seal_tuple!(T0, T1, T2, T3, T4, T5);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);

// ---------------------------------------------------------------------------
// EventError — pardosa-events canonical error surface (GEN-0039)
// ---------------------------------------------------------------------------
//
// 11-variant `repr(u8)` enum with literal discriminants 0..=10. The in-house
// canonical encoding (GEN-0035) emits the discriminant byte as the entire
// payload for these unit-like variants: byte-1 of any encoded `EventError`
// equals the discriminant value. Variant ordering and discriminants are
// frozen as a wire contract — appending new variants is permitted at
// discriminant 11+ in a forward-compatible (Tier-A) revision; renumbering
// is a breaking change requiring a superseding ADR.
//
// EventError is *additive* in v2 — `pardosa_encoding::DecodeError` is the
// existing decoder-local surface, retained while `Decode` trait-signature
// migration is carved to a follow-up sub-mission (C2 / `adr-fmt-vggv`).
// `From<pardosa_encoding::DecodeError> for EventError` bridges decoder
// failures into the wider event-error space for call sites that adopt
// EventError directly.

/// Canonical event-level error surface for pardosa.
///
/// `repr(u8)` with literal discriminants pinned 0..=10. The in-house
/// canonical encoding emits a single byte equal to the discriminant for
/// each variant, per GEN-0039 (F4 wire contract: byte-1 of an encoded
/// `EventError` equals the discriminant value).
///
/// Variant ordering and discriminant values are part of the wire
/// contract — see GEN-0039.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum EventError {
    /// Caller-supplied data violated a documented input invariant.
    InvalidInput = 0,
    /// The addressed entity does not exist.
    NotFound = 1,
    /// The operation conflicts with the current state (e.g. version mismatch,
    /// duplicate key, concurrent write race).
    Conflict = 2,
    /// Caller is not authenticated.
    Unauthorized = 3,
    /// Caller is authenticated but lacks permission for the operation.
    PermissionDenied = 4,
    /// A required dependency is temporarily unavailable; retry may succeed.
    Unavailable = 5,
    /// The operation did not complete within its deadline.
    Timeout = 6,
    /// An internal invariant was violated. Carries no caller-actionable
    /// detail; surface only as an opaque failure.
    Internal = 7,
    /// A resource quota or limit was exceeded (memory, message size, rate).
    ResourceExhausted = 8,
    /// The operation was explicitly cancelled before completion.
    Cancelled = 9,
    /// Underlying storage reported irrecoverable data loss for the
    /// affected entity.
    DataLoss = 10,
}

impl EventError {
    /// Return the wire discriminant byte for this variant.
    ///
    /// Equivalent to the first (and only) byte of `EventError::encode`.
    /// Pinned by GEN-0039; renumbering is a breaking change.
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        // `repr(u8)` makes the cast a no-op at the bit level.
        self as u8
    }
}

impl pardosa_encoding::Encode for EventError {
    fn encode(&self, out: &mut Vec<u8>) {
        // Single byte per GEN-0039 F4 wire contract. `repr(u8)` makes
        // `self.discriminant()` bit-identical to the variant's pinned
        // discriminant.
        out.push(self.discriminant());
    }
}

impl From<pardosa_encoding::DecodeError> for EventError {
    /// Bridge decoder-local failures into the wider event-error space.
    ///
    /// Every `DecodeError` variant maps to `EventError::InvalidInput`:
    /// from the caller's perspective a malformed encoded payload is a
    /// caller-side input defect, not an internal invariant violation.
    /// Call sites that want a finer mapping can match `DecodeError`
    /// directly before bridging.
    ///
    /// This bridge is the v2 transition mechanism. Full migration of
    /// the `Decode` trait signature to return `EventError` directly is
    /// carved to sub-mission C2 (`adr-fmt-vggv`).
    fn from(_err: pardosa_encoding::DecodeError) -> Self {
        EventError::InvalidInput
    }
}

// ---------------------------------------------------------------------------
// Timestamp — event time newtype (GEN-0039)
// ---------------------------------------------------------------------------

use core::num::NonZeroU64;

/// Event timestamp as non-zero epoch nanoseconds.
///
/// `NonZeroU64` is load-bearing: `Option<Timestamp>` is the same size as
/// `Timestamp` (niche optimisation) and zero is reserved as a sentinel
/// for "unset" at the option layer rather than wasting a representable
/// value inside the newtype. Nanosecond granularity covers ~584 years
/// of unsigned range from any chosen epoch; that headroom is the smallest
/// resolution that survives high-frequency event interleaving without
/// loss, and dropping to milliseconds would lose ordering for sub-ms
/// events. See GEN-0039 for the full representation rationale.
///
/// Epoch convention is documented in GEN-0039 (UNIX epoch by default).
/// No `Default` impl is provided — "zero time" is meaningless for an
/// event, and `NonZeroU64` makes an accidental zero unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(NonZeroU64);

impl Timestamp {
    /// Construct a `Timestamp` from a raw epoch-nanosecond count.
    ///
    /// Returns `None` if `nanos == 0` — zero is reserved as the
    /// `Option<Timestamp>::None` sentinel via the `NonZeroU64` niche.
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Option<Self> {
        match NonZeroU64::new(nanos) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }

    /// Return the underlying epoch-nanosecond value.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0.get()
    }
}

// ---------------------------------------------------------------------------
// Validate — invariant-check trait (GEN-0040)
// ---------------------------------------------------------------------------

/// Sync invariant check executed against a constructed value.
///
/// `validate` is intentionally synchronous: command handling is a pure
/// decision (CHE-0008), invariant checks happen in that same pure phase,
/// and admitting `async` here would force every aggregate / event /
/// wrapper validator onto an executor for no semantic gain. Validators
/// must remain side-effect-free and bounded — no I/O, no global state,
/// no allocation that the caller cannot account for. See GEN-0040 for
/// the full rationale; sync-only is a deliberate choice cited against
/// CHE-0008.
///
/// `Error = EventError` is the canonical v2 surface; finer-grained
/// validators are encouraged to construct an `EventError::InvalidInput`
/// and carry diagnostic context out-of-band (logging, tracing) rather
/// than encoding it in the error type.
pub trait Validate {
    /// Validation error type. Defaults to [`EventError`] for the common
    /// case; bounded wrappers (F sub-mission) may use a narrower type
    /// when the error space is genuinely smaller.
    type Error;

    /// Check invariants and return `Ok(())` if the value is well-formed.
    ///
    /// Must be a pure function (CHE-0008): no I/O, no global mutation,
    /// no observable side effects. Implementations should be cheap and
    /// total — long-running or fallible-against-environment checks
    /// belong in command handling proper, not here.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` when the value violates an invariant.
    fn validate(&self) -> Result<(), Self::Error>;
}
