use super::{reject_non_real_f32, reject_non_real_f64};
use crate::error::DomainError;
use crate::genome_safe::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
use pardosa_wire::{EventSafe, Validate};
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RealF32 {
    inner: f32,
}
impl RealF32 {
    #[must_use]
    pub fn get(self) -> f32 {
        self.inner
    }
}
impl TryFrom<f32> for RealF32 {
    type Error = DomainError;
    fn try_from(v: f32) -> Result<Self, DomainError> {
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}
impl pardosa_wire::sealed::Sealed for RealF32 {}
impl EventSafe for RealF32 {}
impl GenomeSafe for RealF32 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"RealF32");
    const SCHEMA_SOURCE: &'static str = "RealF32";
}
impl Validate for RealF32 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        reject_non_real_f32(self.inner).map(|_| ())
    }
}
impl Encode for RealF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl Decode for RealF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let v = f32::decode(d)?;
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RealF64 {
    inner: f64,
}
impl RealF64 {
    #[must_use]
    pub fn get(self) -> f64 {
        self.inner
    }
}
impl TryFrom<f64> for RealF64 {
    type Error = DomainError;
    fn try_from(v: f64) -> Result<Self, DomainError> {
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}
impl pardosa_wire::sealed::Sealed for RealF64 {}
impl EventSafe for RealF64 {}
impl GenomeSafe for RealF64 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"RealF64");
    const SCHEMA_SOURCE: &'static str = "RealF64";
}
impl Validate for RealF64 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        reject_non_real_f64(self.inner).map(|_| ())
    }
}
impl Encode for RealF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl Decode for RealF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let v = f64::decode(d)?;
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}
