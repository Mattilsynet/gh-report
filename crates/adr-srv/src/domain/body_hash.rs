//! `BodyHash` — content-addressed identity for a parsed ADR file.
//! Computed by adr-srv per AFM-0027:R4 (`body_hash` is adr-srv's
//! responsibility; not a field on `adr_fmt::model::AdrRecord`).
//!
//! Algorithm: xxh3-128. Rationale: pardosa-genome already uses xxh3
//! family for schema-fingerprint hashing (`SCHEMA_HASH`); reusing the
//! same hash family across the project keeps the substrate auditable
//! against one set of properties. SHA-256 would be overkill for an
//! integrity check (no adversarial input — adr-srv reads its own
//! corpus) and would cost 16 extra bytes per event payload.
//!
//! Wire shape: 16-byte fixed-size array. Frozen.

use pardosa_genome::GenomeSafe;
use xxhash_rust::xxh3::xxh3_128;

/// xxh3-128 content hash of raw ADR file bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, GenomeSafe)]
pub struct BodyHash([u8; 16]);

impl BodyHash {
    /// Compute the body hash of `bytes` via xxh3-128 with seed 0.
    ///
    /// Determinism: xxh3 is a deterministic hash function; the same
    /// `bytes` always produces the same `BodyHash`. M1.3's idempotency
    /// check (re-scrape: skip unchanged) rests on this.
    #[must_use]
    pub fn compute(bytes: &[u8]) -> Self {
        Self(xxh3_128(bytes).to_le_bytes())
    }

    /// Raw 16-byte representation.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl From<[u8; 16]> for BodyHash {
    fn from(value: [u8; 16]) -> Self {
        Self(value)
    }
}

impl core::fmt::Display for BodyHash {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
