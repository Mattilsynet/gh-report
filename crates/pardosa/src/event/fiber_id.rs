use crate::error::PardosaError;
use pardosa_schema::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode, EventSafe};
use serde::{Deserialize, Serialize};
use std::fmt;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct FiberId(u64);
impl FiberId {
    /// Construct a `FiberId` from a raw `u64`.
    ///
    /// Removed from the default-feature public API (ADR-0017 §D1 /
    /// ADR-0003 §1): `FiberId` is dragline-local; two draglines
    /// instantiated independently produce overlapping values, so a
    /// fabricated `u64` has no meaning outside its dragline's
    /// allocation history. Production mint paths are
    /// [`FiberId::checked_next`] and the internal [`Decode`] impl.
    /// Available under `feature = "test-support"` (and `cfg(test)`)
    /// for raw / tamper fixtures and adopter-facing CLI tooling.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(v: u64) -> Self {
        FiberId(v)
    }
    /// Substrate-internal constructor used by the [`Decode`] impl
    /// — the one legitimate raw-`u64` entry point on the default
    /// feature set.
    #[must_use]
    pub(crate) fn from_decoded(v: u64) -> Self {
        FiberId(v)
    }
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
    /// Return the next `FiberId` value.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberIdOverflow` when incrementing would overflow `u64`.
    pub fn checked_next(self) -> Result<FiberId, PardosaError> {
        self.0
            .checked_add(1)
            .map(FiberId)
            .ok_or(PardosaError::FiberIdOverflow)
    }
}
impl fmt::Display for FiberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Encode for FiberId {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}
impl Decode for FiberId {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        u64::decode(d).map(FiberId::from_decoded)
    }
}
impl pardosa_wire::sealed::Sealed for FiberId {}
impl EventSafe for FiberId {}
impl GenomeSafe for FiberId {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"FiberId");
    const SCHEMA_SOURCE: &'static str = "FiberId";
}
