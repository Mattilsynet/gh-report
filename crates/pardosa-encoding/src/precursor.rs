// ---------------------------------------------------------------------------
// PAR-0021 R1 precursor-hash helper
// ---------------------------------------------------------------------------
//
// BLAKE3 is the pardosa precursor-identity hash. The helper is feature-gated
// so the default no-feature build of `pardosa-encoding` stays dep-free per
// GEN-0041. The hash domain (which event bytes feed in) is the caller's
// responsibility: this helper treats input as opaque bytes and never
// inspects encoding structure. F2c will define the encoding-excluding-hash
// canonicalisation and wire callers accordingly.

/// BLAKE3 hash of canonical event bytes — the precursor identity per PAR-0021 R1.
///
/// Input is the canonical-encoded event bytes EXCLUDING the `precursor_hash`
/// field itself (F2c defines that encoding). This helper treats input as
/// opaque bytes; domain separation is the caller's responsibility.
///
/// Feature-gated under `blake3`; default no-feature build preserves the
/// `#![no_std]` substrate dependency-free per GEN-0041.
#[cfg(feature = "blake3")]
#[must_use]
pub fn precursor_hash_of(event_bytes: &[u8]) -> [u8; 32] {
    blake3::hash(event_bytes).into()
}
