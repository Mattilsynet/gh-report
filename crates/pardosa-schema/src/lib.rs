#![forbid(unsafe_code)]
/// Workspace auto-trait policy macro (mission rescue-pardosa-59y0).
///
/// Compile-time `Send`/`Sync` posture for every public nominal type
/// using **stable built-in marker traits** only (no `auto trait` item,
/// no `negative_impls`). Buckets: `SendSync { T, ... }` verifies
/// `T: Send + Sync`; `SendOnly { T, ... }` verifies `T: Send`
/// (rationale lives at the type's site, e.g. ADR-0014 §F5); `NotSend
/// { T, ... }` is documentation-only (stable Rust cannot assert
/// `!Send`).
///
/// Paired with `tests/auto_trait_policy.rs`, which CI-fails on any
/// unbucketed public type.
macro_rules! assert_auto_traits {
    (
        $(SendSync { $($ss:ty),* $(,)? })? $(SendOnly { $($so:ty),* $(,)? })? $(NotSend {
        $($ns:ty),* $(,)? })?
    ) => {
        const _ : fn () = || { fn __assert_send_sync < T : Send + Sync > () {} fn
        __assert_send < T : Send > () {} $($(__assert_send_sync::<$ss > ();)*)?
        $($(__assert_send::<$so > ();)*)? $($(let _ = ::core::marker::PhantomData::<$ns
        >;)*)? };
    };
}
pub mod bounded;
pub mod char_scalar;
pub mod error;
pub mod floats;
pub mod genome_safe;
pub mod guide;
pub use bounded::{EventBytes, EventString, EventVec, NonEmptyEventString};
pub use char_scalar::CharScalar;
pub use error::DomainError;
pub use floats::{EventF32, EventF64, OrderedF32, OrderedF64, RealF32, RealF64};
pub use genome_safe::{GenomeOrd, GenomeSafe, schema_hash_bytes, schema_hash_combine};
#[cfg(feature = "derive")]
pub use pardosa_derive::GenomeSafe;
pub use pardosa_wire::{Decode, Decoder, Encode, from_bytes, to_vec};
/// Re-exports from `pardosa-wire`. `sealed` is load-bearing: the
/// `pardosa-derive` proc-macro (`#[derive(GenomeSafe)]`) emits
/// `impl <schema>::sealed::Sealed for <Type>` at every derive site,
/// resolving `<schema>` to this crate's path
/// (`crates/pardosa-derive/src/lib.rs:91`). Removing the `sealed`
/// re-export would break every downstream derive consumer.
pub use pardosa_wire::{
    DecodeError, EventSafe, SchemaRejectionCode, StatusCode, Timestamp, Validate, sealed,
};
// AUTO-TRAIT-POLICY-BEGIN
assert_auto_traits! {
    SendSync { EventBytes < 8 >, EventString < 8 >, EventVec < u64, 8 >,
    NonEmptyEventString < 8 >, CharScalar, DomainError, OrderedF32, OrderedF64, RealF32,
    RealF64, EventF32, EventF64, } SendOnly {} NotSend {}
}
// AUTO-TRAIT-POLICY-END
