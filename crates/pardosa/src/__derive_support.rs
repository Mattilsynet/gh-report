//! Re-export hub for `pardosa-derive`-emitted paths. Not a public API.
//!
//! `#[doc(hidden)]` at the parent (`lib.rs`); items are reachable only
//! via macro emit (`::pardosa::__derive_support::Item`) when the
//! consumer names only `pardosa` in `[dependencies]`. Naming
//! `pardosa-schema` / `pardosa-wire` directly continues to work via
//! `proc-macro-crate` fallback; this module is the additive surface
//! enabling the single-dep consumer story.
//!
//! Authoritative enumeration derives from
//! `crates/pardosa-derive/src/{lib.rs,hash.rs,codec.rs}`. Schema side:
//! `EventSafe`, `GenomeOrd`, `GenomeSafe`, `schema_hash_bytes`,
//! `schema_hash_combine`, `sealed`. Wire side: `Decode`,
//! `DecodeError`, `Decoder`, `Encode`. Add new emit names here.
pub use pardosa_schema::{
    EventSafe, GenomeOrd, GenomeSafe, schema_hash_bytes, schema_hash_combine, sealed,
};
pub use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
