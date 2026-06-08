use crate::error::DomainError;
use crate::genome_safe::{GenomeOrd, GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
use pardosa_wire::{EventSafe, Validate};
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct CharScalar {
    inner: char,
}
impl CharScalar {
    #[must_use]
    pub fn get(self) -> char {
        self.inner
    }
}
#[inline]
fn reject_surrogate(cp: u32) -> Result<char, DomainError> {
    char::from_u32(cp).ok_or(DomainError::InvalidChar { code: cp })
}
impl TryFrom<u32> for CharScalar {
    type Error = DomainError;
    fn try_from(cp: u32) -> Result<Self, DomainError> {
        let c = reject_surrogate(cp)?;
        Ok(Self { inner: c })
    }
}
impl TryFrom<char> for CharScalar {
    type Error = DomainError;
    fn try_from(c: char) -> Result<Self, DomainError> {
        Ok(Self { inner: c })
    }
}
impl pardosa_wire::sealed::Sealed for CharScalar {}
impl EventSafe for CharScalar {}
impl GenomeSafe for CharScalar {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"CharScalar");
    const SCHEMA_SOURCE: &'static str = "CharScalar";
}
impl GenomeOrd for CharScalar {}
impl Validate for CharScalar {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        reject_surrogate(u32::from(self.inner)).map(|_| ())
    }
}
impl Encode for CharScalar {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl Decode for CharScalar {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let cp = u32::decode(d)?;
        let c = reject_surrogate(cp)?;
        Ok(Self { inner: c })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use pardosa_wire::ValidationCost;
    use pardosa_wire::{from_bytes, to_vec};
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
        for cp in [0x0041u32, 0x0000, 0x00FF, 0x1_F600, 0x10_FFFF] {
            assert!(
                CharScalar::try_from(cp).is_ok(),
                "expected ok for U+{cp:04X}"
            );
        }
    }
    #[test]
    fn char_scalar_rejects_surrogate_low() {
        assert_eq!(
            CharScalar::try_from(0xD800u32).unwrap_err(),
            DomainError::InvalidChar { code: 0xD800 },
            "must reject U+D800"
        );
    }
    #[test]
    fn char_scalar_rejects_surrogate_high() {
        assert_eq!(
            CharScalar::try_from(0xDFFFu32).unwrap_err(),
            DomainError::InvalidChar { code: 0xDFFF },
            "must reject U+DFFF"
        );
    }
    #[test]
    fn char_scalar_rejects_surrogate_mid() {
        assert_eq!(
            CharScalar::try_from(0xDC00u32).unwrap_err(),
            DomainError::InvalidChar { code: 0xDC00 },
        );
    }
    #[test]
    fn char_scalar_rejects_out_of_range_u32() {
        assert_eq!(
            CharScalar::try_from(0x11_0000u32).unwrap_err(),
            DomainError::InvalidChar { code: 0x11_0000 },
        );
    }
    #[test]
    fn char_scalar_decode_rejects_surrogate() {
        let cp: u32 = 0xD800;
        let mut wire = Vec::new();
        cp.encode(&mut wire);
        let err = from_bytes::<CharScalar>(&wire).unwrap_err();
        assert_eq!(
            err,
            DecodeError::SchemaRejected {
                code: pardosa_wire::SchemaRejectionCode::InvalidChar
            },
            "Decode must reject surrogate with structured detail"
        );
    }
    #[test]
    fn char_scalar_wire_compat_with_raw_char() {
        let c = '🦀';
        let wrapped = to_vec(&CharScalar::try_from(c).unwrap());
        let raw = to_vec(&c);
        assert_eq!(wrapped, raw, "CharScalar wire not byte-identical to char");
    }
    #[test]
    fn char_scalar_validate_cost_cheap() {
        assert_eq!(<CharScalar as Validate>::COST, ValidationCost::Cheap);
    }
    #[test]
    fn char_scalar_validate_green() {
        let w = CharScalar::try_from('€').unwrap();
        assert!(w.validate().is_ok());
    }
    #[test]
    fn char_scalar_genome_ord() {
        fn requires_genome_ord<T: GenomeOrd>() {}
        requires_genome_ord::<CharScalar>();
    }
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
