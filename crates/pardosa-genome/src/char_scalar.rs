//! `CharScalar` wrapper — exactly one Unicode scalar value (GEN-0045:R2).
//!
//! ## Semantic distinction (GEN-0045:R2)
//!
//! | Type | Meaning |
//! |------|---------|
//! | `CharScalar` | Exactly one Unicode scalar; surrogates rejected at construction and decode. |
//! | `EventString<4>` | Up to 4 bytes of UTF-8, potentially several scalars. |
//!
//! Wire format is byte-identical to raw `char` (4-byte LE u32, PM4).
//! `TryFrom<u32>` rejects surrogate codepoints U+D800..=U+DFFF at the boundary.
//! `TryFrom<char>` is provided for ergonomics; Rust's `char` already excludes
//! surrogates at the type level so that path is infallible in practice — the
//! wrapper makes the semantic intent explicit.

use pardosa_encoding::{Decode, Decoder, Encode, EventError};
use pardosa_traits::{EventSafe, Validate, sealed::Sealed};

use crate::genome_safe::{GenomeOrd, GenomeSafe, schema_hash_bytes};

// ---------------------------------------------------------------------------
// CharScalar
// ---------------------------------------------------------------------------

/// `char` wrapper asserting exactly one Unicode scalar value.
///
/// Rejects surrogate codepoints U+D800..=U+DFFF at `TryFrom<u32>` and at
/// `Decode`. Wire byte-identical to raw `char` (4-byte LE u32, PM4).
///
/// Raw `char` retains `GenomeSafe` for fields where the intent is a raw
/// Unicode codepoint without the explicit-scalar contract. See GEN-0045:R2.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct CharScalar {
    inner: char,
}

impl CharScalar {
    /// Return the inner `char` value. Guaranteed to be a valid Unicode scalar
    /// (surrogates excluded).
    #[must_use]
    pub fn get(self) -> char {
        self.inner
    }
}

// U+D800..=U+DFFF are the surrogate range. `char::from_u32` already rejects
// them, but we spell out the contract explicitly so the error path is visible.
#[inline]
fn reject_surrogate(cp: u32) -> Result<char, EventError> {
    char::from_u32(cp).ok_or(EventError::InvalidInput)
}

impl TryFrom<u32> for CharScalar {
    type Error = EventError;
    fn try_from(cp: u32) -> Result<Self, EventError> {
        let c = reject_surrogate(cp)?;
        Ok(Self { inner: c })
    }
}

impl TryFrom<char> for CharScalar {
    type Error = EventError;
    fn try_from(c: char) -> Result<Self, EventError> {
        // Rust's `char` already excludes surrogates; this path is infallible
        // at the Rust type level. The `TryFrom` form is provided so callers
        // have a uniform construction API and the intent is explicit.
        Ok(Self { inner: c })
    }
}

impl Sealed for CharScalar {}
impl EventSafe for CharScalar {}

impl GenomeSafe for CharScalar {
    // Schema hash distinguishes the wrapper from raw char; same wire bytes,
    // different type contract.
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"CharScalar");
    const SCHEMA_SOURCE: &'static str = "CharScalar";
}

impl GenomeOrd for CharScalar {}

impl Validate for CharScalar {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        // Rust `char` invariant guarantees validity; this is belt-and-braces
        // per GEN-0042's S3 pattern (validate arm redundancy for future
        // mutation paths).
        reject_surrogate(u32::from(self.inner)).map(|_| ())
    }
}

impl Encode for CharScalar {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for CharScalar {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let cp = u32::decode(d)?;
        let c = reject_surrogate(cp)?;
        Ok(Self { inner: c })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pardosa_encoding::{from_bytes, to_vec};
    use pardosa_traits::ValidationCost;

    // ---- Construction and acceptance ---------------------------------------

    #[test]
    fn char_scalar_from_char_roundtrip() {
        for c in ['a', 'Z', '\0', '\u{0041}', '\u{1F600}', '\u{10FFFF}'] {
            let w = CharScalar::try_from(c).unwrap();
            let wire = to_vec(&w);
            let back: CharScalar = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "CharScalar roundtrip failed for {c:?}");
        }
    }

    #[test]
    fn char_scalar_from_u32_valid() {
        // Some valid Unicode scalar codepoints.
        for cp in [0x0041u32, 0x0000, 0x00FF, 0x1F600, 0x10FFFF] {
            assert!(
                CharScalar::try_from(cp).is_ok(),
                "expected ok for U+{cp:04X}"
            );
        }
    }

    // ---- Surrogate rejection at TryFrom<u32> -------------------------------

    #[test]
    fn char_scalar_rejects_surrogate_low() {
        // U+D800 — start of surrogate range
        assert_eq!(
            CharScalar::try_from(0xD800u32).unwrap_err(),
            EventError::InvalidInput,
            "must reject U+D800"
        );
    }

    #[test]
    fn char_scalar_rejects_surrogate_high() {
        // U+DFFF — end of surrogate range
        assert_eq!(
            CharScalar::try_from(0xDFFFu32).unwrap_err(),
            EventError::InvalidInput,
            "must reject U+DFFF"
        );
    }

    #[test]
    fn char_scalar_rejects_surrogate_mid() {
        // U+DC00 — low surrogate (inside range)
        assert_eq!(
            CharScalar::try_from(0xDC00u32).unwrap_err(),
            EventError::InvalidInput,
        );
    }

    #[test]
    fn char_scalar_rejects_out_of_range_u32() {
        // U+110000 — one past the Unicode ceiling
        assert_eq!(
            CharScalar::try_from(0x11_0000u32).unwrap_err(),
            EventError::InvalidInput,
        );
    }

    // ---- Surrogate rejection at Decode boundary ----------------------------

    #[test]
    fn char_scalar_decode_rejects_surrogate() {
        // Manually encode a surrogate codepoint as u32 LE (4 bytes).
        let cp: u32 = 0xD800;
        let mut wire = Vec::new();
        cp.encode(&mut wire);
        let err = from_bytes::<CharScalar>(&wire).unwrap_err();
        assert_eq!(
            err,
            EventError::InvalidInput,
            "Decode must reject surrogate"
        );
    }

    // ---- Wire-compat PM4 lock ----------------------------------------------

    #[test]
    fn char_scalar_wire_compat_with_raw_char() {
        // PM4: CharScalar wire bytes must be byte-identical to raw char.
        let c = '🦀';
        let wrapped = to_vec(&CharScalar::try_from(c).unwrap());
        let raw = to_vec(&c);
        assert_eq!(wrapped, raw, "CharScalar wire not byte-identical to char");
    }

    // ---- Validate ----------------------------------------------------------

    #[test]
    fn char_scalar_validate_cost_cheap() {
        assert_eq!(<CharScalar as Validate>::COST, ValidationCost::Cheap);
    }

    #[test]
    fn char_scalar_validate_green() {
        let w = CharScalar::try_from('€').unwrap();
        assert!(w.validate().is_ok());
    }

    // ---- GenomeOrd ---------------------------------------------------------

    #[test]
    fn char_scalar_genome_ord() {
        // CharScalar must implement GenomeOrd (map-key eligible).
        fn requires_genome_ord<T: GenomeOrd>() {}
        requires_genome_ord::<CharScalar>();
    }

    // ---- Schema hash differs from raw char ---------------------------------

    #[test]
    fn char_scalar_schema_hash_differs_from_char() {
        use crate::genome_safe::GenomeSafe;
        assert_ne!(
            <CharScalar as GenomeSafe>::SCHEMA_HASH,
            <char as GenomeSafe>::SCHEMA_HASH,
            "CharScalar schema hash must differ from raw char"
        );
    }
}
