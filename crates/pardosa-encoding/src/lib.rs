//! In-house canonical encoding for pardosa events (GEN-0035).
//!
//! The wire format is a deterministic sequential canonical encoding
//! (LE primitives, length-prefixed variable-width data, `repr(u8)`
//! enum discriminants) owned by the workspace so we control the spec,
//! the sealing, and the decoder cap semantics. This crate provides the
//! substrate ([`Encode`], [`Decode`], [`EventError`], primitive impls);
//! the sealed `EventSafe` trait stack lives in `pardosa-traits` as
//! `EventSafe: Encode + Sealed`.
//!
//! See `docs/adr/genome/GEN-0035-in-house-canonical-encoding.md`.

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

mod decoder;
pub use decoder::{DEFAULT_DECODE_CAP, Decoder};

mod error;
pub use error::EventError;

mod traits;
pub use traits::{Decode, Encode, from_bytes, from_bytes_with_cap, to_vec};

mod primitives;

mod composites;

mod foreign;

mod precursor;
#[cfg(feature = "blake3")]
pub use precursor::precursor_hash_of;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
