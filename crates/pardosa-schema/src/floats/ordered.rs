use super::{reject_non_real_f32, reject_non_real_f64};
use crate::error::DomainError;
use crate::genome_safe::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
use pardosa_wire::{EventSafe, Validate};
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OrderedF32 {
    inner: f32,
}
impl OrderedF32 {
    #[must_use]
    pub fn get(self) -> f32 {
        self.inner
    }
}
impl TryFrom<f32> for OrderedF32 {
    type Error = DomainError;
    fn try_from(v: f32) -> Result<Self, DomainError> {
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}
impl PartialEq for OrderedF32 {
    fn eq(&self, other: &Self) -> bool {
        self.inner.total_cmp(&other.inner).is_eq()
    }
}
impl Eq for OrderedF32 {}
impl PartialOrd for OrderedF32 {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrderedF32 {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.inner.total_cmp(&other.inner)
    }
}
impl core::hash::Hash for OrderedF32 {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.inner.to_bits().hash(state);
    }
}
impl pardosa_wire::sealed::Sealed for OrderedF32 {}
impl EventSafe for OrderedF32 {}
impl GenomeSafe for OrderedF32 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"OrderedF32");
    const SCHEMA_SOURCE: &'static str = "OrderedF32";
}
impl crate::genome_safe::GenomeOrd for OrderedF32 {}
impl Validate for OrderedF32 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        reject_non_real_f32(self.inner).map(|_| ())
    }
}
impl Encode for OrderedF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl Decode for OrderedF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let v = f32::decode(d)?;
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OrderedF64 {
    inner: f64,
}
impl OrderedF64 {
    #[must_use]
    pub fn get(self) -> f64 {
        self.inner
    }
}
impl TryFrom<f64> for OrderedF64 {
    type Error = DomainError;
    fn try_from(v: f64) -> Result<Self, DomainError> {
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}
impl PartialEq for OrderedF64 {
    fn eq(&self, other: &Self) -> bool {
        self.inner.total_cmp(&other.inner).is_eq()
    }
}
impl Eq for OrderedF64 {}
impl PartialOrd for OrderedF64 {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrderedF64 {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.inner.total_cmp(&other.inner)
    }
}
impl core::hash::Hash for OrderedF64 {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.inner.to_bits().hash(state);
    }
}
impl pardosa_wire::sealed::Sealed for OrderedF64 {}
impl EventSafe for OrderedF64 {}
impl GenomeSafe for OrderedF64 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"OrderedF64");
    const SCHEMA_SOURCE: &'static str = "OrderedF64";
}
impl crate::genome_safe::GenomeOrd for OrderedF64 {}
impl Validate for OrderedF64 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        reject_non_real_f64(self.inner).map(|_| ())
    }
}
impl Encode for OrderedF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl Decode for OrderedF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let v = f64::decode(d)?;
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}
