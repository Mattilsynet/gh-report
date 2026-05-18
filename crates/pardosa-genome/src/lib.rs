//! pardosa-genome — Binary serialization with zero-copy reads and serde integration.
//!
//! Combines `FlatBuffers`' zero-copy read performance with RON's full algebraic
//! data model. Standard serde with a lightweight `GenomeSafe` marker derive.
//!
//! # Status
//!
//! Phase 1 implementation: crate scaffold, `GenomeSafe` trait, format constants,
//! config types, error catalog.

#![forbid(unsafe_code)]

pub mod bounded;
pub mod config;
pub mod error;
pub mod file;
pub mod floats;
pub mod format;
pub mod genome_safe;

// Bounded wrapper types (GEN-0042). Per-field MAX enforcement layered on
// top of the GEN-0035:R8 decoder cap; invariant-only wire format.
pub use bounded::{EventBytes, EventString, EventVec, NonEmptyEventString};

// Float-tier wrapper family (GEN-0045:R1). Invariant-only wrappers over
// f32/f64; wire byte-identical to inner type (PM4).
pub use floats::{FiniteF32, FiniteF64, OrderedF32, OrderedF64, RealF32, RealF64};

// Re-exports
pub use config::{Compression, DecodeOptions, PageClass};
pub use error::{DeError, FileError, SerError};
pub use file::{Reader, Writer};
pub use genome_safe::{GenomeOrd, GenomeSafe, schema_hash_bytes, schema_hash_combine};
// EventSafe + sealed module re-exported so downstream `use pardosa_genome::*`
// keeps resolving and the derive macro's emitted paths
// (`::pardosa_genome::sealed::Sealed`) work without users depending on
// `pardosa-traits` directly.
//
// EventError, Timestamp, Validate are the typing-core surface introduced
// in sub-mission C (GEN-0038/0039/0040). Alphabetised; explicit (no glob).
pub use pardosa_traits::{EventError, EventSafe, Timestamp, Validate, sealed};

// Encode/Decode re-exported from pardosa-encoding so the derive macro's
// emitted `::pardosa_genome::{Encode, Decode}` paths resolve in downstream
// user code and trybuild fixtures without a direct pardosa-encoding dep.
// Mirrors the EventSafe re-export pattern above.
//
// `EventError` is the canonical event-level error and the return type of
// [`Decode::decode`]; decoder-local failure modes (truncated input, cap
// exceeded, unknown discriminant, invalid UTF-8, non-canonical map,
// trailing bytes) all collapse to `EventError::InvalidInput`. Granularity
// loss is intentional — call sites get a single uniform error surface;
// finer diagnosis is out-of-band (logging/tracing). See GEN-0040.
pub use pardosa_encoding::{Decode, Decoder, Encode, from_bytes, to_vec};

// Re-export derive macro when the `derive` feature is enabled.
// Derive macros and traits live in different namespaces — both resolve
// correctly when imported via `use pardosa_genome::GenomeSafe`.
#[cfg(feature = "derive")]
pub use pardosa_derive::GenomeSafe;
