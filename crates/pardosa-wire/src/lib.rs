//! `no_std` byte-canonical encode/decode substrate for pardosa payloads.
//!
//! The lowest ring of the payload stack: the [`Encode`]/[`Decode`] traits
//! and their [`to_vec`]/[`from_bytes`] helpers define a deterministic,
//! one-value-one-byte-string wire form, with [`Decoder`] enforcing a
//! decode cap against hostile input. [`EventSafe`] is sealed here so only
//! workspace-blessed types reach an event payload; [`laws`] exports the
//! round-trip/canonicity property checks adopters run over their own
//! types. `no_std` + `alloc` only — no runtime, no I/O.
#![forbid(unsafe_code)]
#![no_std]
extern crate alloc;
/// Workspace auto-trait policy macro (mission rescue-pardosa-59y0).
///
/// `no_std`-safe: uses only the **stable built-in `Send`/`Sync`** bounds
/// from the prelude and `core::marker::PhantomData`. No custom
/// `auto trait`, no `#![feature(auto_traits)]`, no `negative_impls`,
/// no `impl !Send`/`impl !Sync`. See pardosa-schema's `lib.rs` for the
/// full doctrine; this is the verbatim copy adapted for `no_std`.
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
mod decoder;
pub use decoder::{DEFAULT_DECODE_CAP, Decoder};
mod error;
pub use error::{DecodeError, SchemaRejectionCode, StatusCode};
pub mod sealed;
mod traits;
pub use traits::{Decode, Encode, EventSafe, from_bytes, from_bytes_with_cap, to_vec};
mod blankets;
mod composites;
mod foreign;
mod primitives;
mod try_encode;
pub use try_encode::{EncodeOverflow, TryEncode, try_encode_len_prefix, try_to_vec};
mod validate;
pub use validate::{Timestamp, Validate, ValidationCost};
mod precursor;
#[cfg(feature = "blake3")]
pub use precursor::precursor_hash_of;
pub mod laws;
#[cfg(test)]
mod tests;
// AUTO-TRAIT-POLICY-BEGIN
assert_auto_traits! {
    SendSync { Decoder <'static >, DecodeError, EncodeOverflow, SchemaRejectionCode,
    StatusCode, Timestamp, ValidationCost, } SendOnly {} NotSend {}
}
// AUTO-TRAIT-POLICY-END
