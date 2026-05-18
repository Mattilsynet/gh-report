//! Float-tier wrapper family (GEN-0045:R1).
//!
//! Three tiers of float wrappers over `f32` / `f64`, each expressing a
//! progressively stricter invariant at construction and decode time:
//!
//! | Wrapper | Rejects | Extra |
//! |---------|---------|-------|
//! | [`FiniteF32`] / [`FiniteF64`] | NaN | ±∞ accepted |
//! | [`RealF32`] / [`RealF64`]     | NaN, ±∞, subnormals | -0 normalised → +0 |
//! | [`OrderedF32`] / [`OrderedF64`] | NaN, ±∞, subnormals | `Ord` via `total_cmp` |
//!
//! Wire format is byte-identical to the inner `f32` / `f64` (4 / 8 bytes LE
//! IEEE-754, per GEN-0035:R1 and PM4). Wrappers express invariants, not a
//! distinct encoding.
//!
//! Raw `f32` / `f64` retain `GenomeSafe` for fields that deliberately carry
//! IEEE-754 divergence signal (ML weights, NaN-boxing, etc.). Use these
//! wrappers for new event fields that must be well-behaved at the boundary.

use pardosa_encoding::{Decode, Decoder, Encode, EventError};
use pardosa_traits::{EventSafe, Validate, sealed::Sealed};

use crate::genome_safe::{GenomeSafe, schema_hash_bytes};

// ---------------------------------------------------------------------------
// Helpers — shared validation logic
// ---------------------------------------------------------------------------

#[inline]
fn reject_nan_f32(v: f32) -> Result<(), EventError> {
    if v.is_nan() {
        return Err(EventError::InvalidInput);
    }
    Ok(())
}

#[inline]
fn reject_nan_f64(v: f64) -> Result<(), EventError> {
    if v.is_nan() {
        return Err(EventError::InvalidInput);
    }
    Ok(())
}

#[inline]
fn reject_non_real_f32(v: f32) -> Result<f32, EventError> {
    // Rejects NaN, ±∞, and subnormals. Normalises -0.0 → +0.0.
    // Subnormal check: is_subnormal() is true for denormalised values;
    // -0.0 is not subnormal but must be normalised to maintain byte-level
    // determinism for map keys (total_cmp distinguishes ±0 — we collapse
    // to +0 so the encoding is canonical).
    if v.is_nan() || v.is_infinite() || v.is_subnormal() {
        return Err(EventError::InvalidInput);
    }
    // Normalise -0.0 → +0.0 via bit comparison.
    if v == 0.0_f32 && v.is_sign_negative() {
        return Ok(0.0_f32);
    }
    Ok(v)
}

#[inline]
fn reject_non_real_f64(v: f64) -> Result<f64, EventError> {
    if v.is_nan() || v.is_infinite() || v.is_subnormal() {
        return Err(EventError::InvalidInput);
    }
    if v == 0.0_f64 && v.is_sign_negative() {
        return Ok(0.0_f64);
    }
    Ok(v)
}

// ---------------------------------------------------------------------------
// FiniteF32 — rejects NaN; ±∞ accepted (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f32` wrapper that rejects `NaN` at construction and decode.
///
/// `±∞` is accepted — "Finite" in this context means "not NaN"; callers that
/// want to exclude infinities should use [`RealF32`]. Wire byte-identical to
/// `f32` (4 bytes LE IEEE-754, PM4). See GEN-0045:R1.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FiniteF32 {
    inner: f32,
}

impl FiniteF32 {
    /// Return the inner `f32` value.
    #[must_use]
    pub fn get(self) -> f32 {
        self.inner
    }
}

impl TryFrom<f32> for FiniteF32 {
    type Error = EventError;
    fn try_from(v: f32) -> Result<Self, EventError> {
        reject_nan_f32(v)?;
        Ok(Self { inner: v })
    }
}

impl Sealed for FiniteF32 {}
impl EventSafe for FiniteF32 {}

impl GenomeSafe for FiniteF32 {
    // Schema hash distinguishes the wrapper from raw f32; the wrapper is a
    // different type contract even though the wire bytes are identical.
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"FiniteF32");
    const SCHEMA_SOURCE: &'static str = "FiniteF32";
}

impl Validate for FiniteF32 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_nan_f32(self.inner)
    }
}

impl Encode for FiniteF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for FiniteF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f32::decode(d)?;
        reject_nan_f32(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// FiniteF64 — rejects NaN; ±∞ accepted (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f64` wrapper that rejects `NaN` at construction and decode.
///
/// See [`FiniteF32`] for the tier rationale. Wire byte-identical to `f64`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct FiniteF64 {
    inner: f64,
}

impl FiniteF64 {
    /// Return the inner `f64` value.
    #[must_use]
    pub fn get(self) -> f64 {
        self.inner
    }
}

impl TryFrom<f64> for FiniteF64 {
    type Error = EventError;
    fn try_from(v: f64) -> Result<Self, EventError> {
        reject_nan_f64(v)?;
        Ok(Self { inner: v })
    }
}

impl Sealed for FiniteF64 {}
impl EventSafe for FiniteF64 {}

impl GenomeSafe for FiniteF64 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"FiniteF64");
    const SCHEMA_SOURCE: &'static str = "FiniteF64";
}

impl Validate for FiniteF64 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_nan_f64(self.inner)
    }
}

impl Encode for FiniteF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for FiniteF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f64::decode(d)?;
        reject_nan_f64(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// RealF32 — rejects NaN, ±∞, subnormals; normalises -0 → +0 (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f32` wrapper that rejects `NaN`, `±∞`, and subnormals at construction
/// and decode. `-0.0` is normalised to `+0.0` on construction.
///
/// Wire byte-identical to `f32` for all accepted values. The normalisation
/// of `-0.0` ensures byte-level determinism: two `RealF32` values that
/// compare equal via `==` also produce identical wire bytes. See GEN-0045:R1.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RealF32 {
    inner: f32,
}

impl RealF32 {
    /// Return the inner `f32` value. Guaranteed non-NaN, non-infinite, non-subnormal.
    #[must_use]
    pub fn get(self) -> f32 {
        self.inner
    }
}

impl TryFrom<f32> for RealF32 {
    type Error = EventError;
    fn try_from(v: f32) -> Result<Self, EventError> {
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}

impl Sealed for RealF32 {}
impl EventSafe for RealF32 {}

impl GenomeSafe for RealF32 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"RealF32");
    const SCHEMA_SOURCE: &'static str = "RealF32";
}

impl Validate for RealF32 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_non_real_f32(self.inner).map(|_| ())
    }
}

impl Encode for RealF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for RealF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f32::decode(d)?;
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// RealF64 — rejects NaN, ±∞, subnormals; normalises -0 → +0 (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f64` wrapper that rejects `NaN`, `±∞`, and subnormals at construction
/// and decode. `-0.0` is normalised to `+0.0` on construction.
///
/// See [`RealF32`] for the tier rationale. Wire byte-identical to `f64`.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct RealF64 {
    inner: f64,
}

impl RealF64 {
    /// Return the inner `f64` value. Guaranteed non-NaN, non-infinite, non-subnormal.
    #[must_use]
    pub fn get(self) -> f64 {
        self.inner
    }
}

impl TryFrom<f64> for RealF64 {
    type Error = EventError;
    fn try_from(v: f64) -> Result<Self, EventError> {
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}

impl Sealed for RealF64 {}
impl EventSafe for RealF64 {}

impl GenomeSafe for RealF64 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"RealF64");
    const SCHEMA_SOURCE: &'static str = "RealF64";
}

impl Validate for RealF64 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_non_real_f64(self.inner).map(|_| ())
    }
}

impl Encode for RealF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for RealF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f64::decode(d)?;
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// OrderedF32 — RealF32 + Eq + Hash + Ord via total_cmp (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f32` wrapper with the same rejection policy as [`RealF32`] plus a total
/// ordering via [`f32::total_cmp`].
///
/// `Ord` is sound because:
/// 1. NaN is rejected at construction and decode — no NaN can ever inhabit
///    this wrapper, so `total_cmp`'s NaN-handling branch is unreachable.
/// 2. `±∞` is rejected at construction and decode.
/// 3. `-0.0` is normalised to `+0.0`, so `total_cmp`'s ±0 distinction
///    (which would break `Eq`) cannot arise; all zeros are `+0.0`.
/// 4. With NaN, infinities, and negative-zero excluded, `total_cmp` is a
///    consistent total order that agrees with `PartialOrd` for all inhabitants.
///
/// `Hash` is consistent with `Eq` because no two equal `OrderedF32` values
/// differ in bit pattern after the normalisation above.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OrderedF32 {
    inner: f32,
}

impl OrderedF32 {
    /// Return the inner `f32` value. Guaranteed non-NaN, non-infinite, non-subnormal, non-negative-zero.
    #[must_use]
    pub fn get(self) -> f32 {
        self.inner
    }
}

impl TryFrom<f32> for OrderedF32 {
    type Error = EventError;
    fn try_from(v: f32) -> Result<Self, EventError> {
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
        // total_cmp is sound here: NaN and ±∞ are rejected at construction;
        // -0.0 is normalised to +0.0. All inhabitants are normal finite
        // non-negative-zero floats, so total_cmp agrees with PartialOrd.
        self.inner.total_cmp(&other.inner)
    }
}

impl core::hash::Hash for OrderedF32 {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // Bit-pattern hash. Safe because -0.0 has been normalised to +0.0,
        // so two equal OrderedF32 values always carry the same bit pattern.
        self.inner.to_bits().hash(state);
    }
}

impl Sealed for OrderedF32 {}
impl EventSafe for OrderedF32 {}

impl GenomeSafe for OrderedF32 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"OrderedF32");
    const SCHEMA_SOURCE: &'static str = "OrderedF32";
}

impl crate::genome_safe::GenomeOrd for OrderedF32 {}

impl Validate for OrderedF32 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_non_real_f32(self.inner).map(|_| ())
    }
}

impl Encode for OrderedF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for OrderedF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f32::decode(d)?;
        let v = reject_non_real_f32(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// OrderedF64 — RealF64 + Eq + Hash + Ord via total_cmp (GEN-0045:R1)
// ---------------------------------------------------------------------------

/// `f64` wrapper with the same rejection policy as [`RealF64`] plus a total
/// ordering via [`f64::total_cmp`].
///
/// See [`OrderedF32`] for the soundness argument; it applies symmetrically.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct OrderedF64 {
    inner: f64,
}

impl OrderedF64 {
    /// Return the inner `f64` value. Guaranteed non-NaN, non-infinite, non-subnormal, non-negative-zero.
    #[must_use]
    pub fn get(self) -> f64 {
        self.inner
    }
}

impl TryFrom<f64> for OrderedF64 {
    type Error = EventError;
    fn try_from(v: f64) -> Result<Self, EventError> {
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

impl Sealed for OrderedF64 {}
impl EventSafe for OrderedF64 {}

impl GenomeSafe for OrderedF64 {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"OrderedF64");
    const SCHEMA_SOURCE: &'static str = "OrderedF64";
}

impl crate::genome_safe::GenomeOrd for OrderedF64 {}

impl Validate for OrderedF64 {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        reject_non_real_f64(self.inner).map(|_| ())
    }
}

impl Encode for OrderedF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl Decode for OrderedF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let v = f64::decode(d)?;
        let v = reject_non_real_f64(v)?;
        Ok(Self { inner: v })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use core::f32::consts::PI as PI_F32;
    use core::f64::consts::PI as PI_F64;
    use pardosa_encoding::{from_bytes, to_vec};
    use pardosa_traits::ValidationCost;

    // ---- FiniteF32 ---------------------------------------------------------

    #[test]
    fn finite_f32_roundtrip() {
        for &v in &[
            PI_F32,
            0.0_f32,
            -0.0_f32,
            f32::MIN,
            f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            let w = FiniteF32::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: FiniteF32 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "FiniteF32 roundtrip failed for {v}");
        }
    }

    #[test]
    fn finite_f32_rejects_nan() {
        assert_eq!(
            FiniteF32::try_from(f32::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn finite_f32_accepts_infinities() {
        // GEN-0045:R1: Finite tier rejects NaN only; ±∞ are accepted.
        assert!(FiniteF32::try_from(f32::INFINITY).is_ok());
        assert!(FiniteF32::try_from(f32::NEG_INFINITY).is_ok());
    }

    #[test]
    fn finite_f32_validation_cost() {
        assert_eq!(<FiniteF32 as Validate>::COST, ValidationCost::Cheap);
    }

    // ---- FiniteF64 ---------------------------------------------------------

    #[test]
    fn finite_f64_roundtrip() {
        for &v in &[
            PI_F64,
            0.0_f64,
            -0.0_f64,
            f64::MIN,
            f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            let w = FiniteF64::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: FiniteF64 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "FiniteF64 roundtrip failed for {v}");
        }
    }

    #[test]
    fn finite_f64_rejects_nan() {
        assert_eq!(
            FiniteF64::try_from(f64::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn finite_f64_accepts_infinities() {
        assert!(FiniteF64::try_from(f64::INFINITY).is_ok());
        assert!(FiniteF64::try_from(f64::NEG_INFINITY).is_ok());
    }

    // ---- RealF32 -----------------------------------------------------------

    #[test]
    fn real_f32_roundtrip() {
        for &v in &[PI_F32, 0.0_f32, f32::MIN, f32::MAX] {
            let w = RealF32::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: RealF32 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "RealF32 roundtrip failed for {v}");
        }
    }

    #[test]
    fn real_f32_rejects_nan() {
        assert_eq!(
            RealF32::try_from(f32::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f32_rejects_infinities() {
        assert_eq!(
            RealF32::try_from(f32::INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
        assert_eq!(
            RealF32::try_from(f32::NEG_INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f32_rejects_subnormal() {
        // f32::MIN_POSITIVE / 2 is subnormal.
        let sub = f32::MIN_POSITIVE / 2.0;
        assert!(sub.is_subnormal(), "test setup: expected subnormal");
        assert_eq!(
            RealF32::try_from(sub).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f32_normalises_negative_zero() {
        let w = RealF32::try_from(-0.0_f32).unwrap();
        // After normalisation the inner must be +0.0 (positive bit pattern).
        assert!(
            !w.get().is_sign_negative(),
            "RealF32 must normalise -0 to +0"
        );
    }

    // ---- RealF64 -----------------------------------------------------------

    #[test]
    fn real_f64_roundtrip() {
        for &v in &[PI_F64, 0.0_f64, f64::MIN, f64::MAX] {
            let w = RealF64::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: RealF64 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "RealF64 roundtrip failed for {v}");
        }
    }

    #[test]
    fn real_f64_rejects_nan() {
        assert_eq!(
            RealF64::try_from(f64::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f64_rejects_infinities() {
        assert_eq!(
            RealF64::try_from(f64::INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
        assert_eq!(
            RealF64::try_from(f64::NEG_INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f64_rejects_subnormal() {
        let sub = f64::MIN_POSITIVE / 2.0;
        assert!(sub.is_subnormal());
        assert_eq!(
            RealF64::try_from(sub).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn real_f64_normalises_negative_zero() {
        let w = RealF64::try_from(-0.0_f64).unwrap();
        assert!(!w.get().is_sign_negative());
    }

    // ---- OrderedF32 --------------------------------------------------------

    #[test]
    fn ordered_f32_roundtrip() {
        for &v in &[PI_F32, 0.0_f32, f32::MIN, f32::MAX, -1.0_f32, 1.0_f32] {
            let w = OrderedF32::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: OrderedF32 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "OrderedF32 roundtrip failed for {v}");
        }
    }

    #[test]
    fn ordered_f32_rejects_nan() {
        assert_eq!(
            OrderedF32::try_from(f32::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f32_rejects_infinities() {
        assert_eq!(
            OrderedF32::try_from(f32::INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
        assert_eq!(
            OrderedF32::try_from(f32::NEG_INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f32_rejects_subnormal() {
        let sub = f32::MIN_POSITIVE / 2.0;
        assert!(sub.is_subnormal());
        assert_eq!(
            OrderedF32::try_from(sub).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f32_total_ordering() {
        // Sorted vec including negative values, zero, and large values.
        // -0.0 normalises to +0.0 so the sort is unambiguous.
        let mut vals: Vec<OrderedF32> = [-1.0_f32, 0.0, 1.0, 100.0, -100.0, f32::MIN, f32::MAX]
            .iter()
            .map(|&v| OrderedF32::try_from(v).unwrap())
            .collect();
        vals.sort();
        let raw: Vec<f32> = vals.iter().map(|w| w.get()).collect();
        // Verify monotone.
        for win in raw.windows(2) {
            assert!(
                win[0] <= win[1],
                "ordering violated: {} > {}",
                win[0],
                win[1]
            );
        }
    }

    #[test]
    fn ordered_f32_negative_zero_normalised() {
        let pos = OrderedF32::try_from(0.0_f32).unwrap();
        let neg = OrderedF32::try_from(-0.0_f32).unwrap();
        // After normalisation both are +0.0; they must be equal.
        assert_eq!(pos, neg);
        assert_eq!(pos.cmp(&neg), core::cmp::Ordering::Equal);
    }

    // ---- OrderedF64 --------------------------------------------------------

    #[test]
    fn ordered_f64_roundtrip() {
        for &v in &[PI_F64, 0.0_f64, f64::MIN, f64::MAX, -1.0_f64, 1.0_f64] {
            let w = OrderedF64::try_from(v).unwrap();
            let wire = to_vec(&w);
            let back: OrderedF64 = from_bytes(&wire).unwrap();
            assert_eq!(w, back, "OrderedF64 roundtrip failed for {v}");
        }
    }

    #[test]
    fn ordered_f64_rejects_nan() {
        assert_eq!(
            OrderedF64::try_from(f64::NAN).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f64_rejects_infinities() {
        assert_eq!(
            OrderedF64::try_from(f64::INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
        assert_eq!(
            OrderedF64::try_from(f64::NEG_INFINITY).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f64_rejects_subnormal() {
        let sub = f64::MIN_POSITIVE / 2.0;
        assert!(sub.is_subnormal());
        assert_eq!(
            OrderedF64::try_from(sub).unwrap_err(),
            EventError::InvalidInput
        );
    }

    #[test]
    fn ordered_f64_total_ordering() {
        let mut vals: Vec<OrderedF64> = [-1.0_f64, 0.0, 1.0, 100.0, -100.0, f64::MIN, f64::MAX]
            .iter()
            .map(|&v| OrderedF64::try_from(v).unwrap())
            .collect();
        vals.sort();
        let raw: Vec<f64> = vals.iter().map(|w| w.get()).collect();
        for win in raw.windows(2) {
            assert!(
                win[0] <= win[1],
                "ordering violated: {} > {}",
                win[0],
                win[1]
            );
        }
    }

    #[test]
    fn ordered_f64_negative_zero_normalised() {
        let pos = OrderedF64::try_from(0.0_f64).unwrap();
        let neg = OrderedF64::try_from(-0.0_f64).unwrap();
        assert_eq!(pos, neg);
        assert_eq!(pos.cmp(&neg), core::cmp::Ordering::Equal);
    }

    // ---- Cross-type ValidationCost -----------------------------------------

    #[test]
    fn all_wrappers_validation_cost_cheap() {
        assert_eq!(<FiniteF32 as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<FiniteF64 as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<RealF32 as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<RealF64 as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<OrderedF32 as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<OrderedF64 as Validate>::COST, ValidationCost::Cheap);
    }

    // ---- Wire-compat PM4 lock ----------------------------------------------

    #[test]
    fn wire_compat_finite_f32_vs_raw() {
        // PM4: FiniteF32 wire bytes must be byte-identical to raw f32.
        let v = PI_F32;
        let wrapped = to_vec(&FiniteF32::try_from(v).unwrap());
        let raw = to_vec(&v);
        assert_eq!(wrapped, raw, "FiniteF32 wire not byte-identical to f32");
    }

    #[test]
    fn wire_compat_real_f64_vs_raw() {
        let v = PI_F64;
        let wrapped = to_vec(&RealF64::try_from(v).unwrap());
        let raw = to_vec(&v);
        assert_eq!(wrapped, raw, "RealF64 wire not byte-identical to f64");
    }

    #[test]
    fn wire_compat_ordered_f32_vs_raw() {
        let v = 1.23_f32;
        let wrapped = to_vec(&OrderedF32::try_from(v).unwrap());
        let raw = to_vec(&v);
        assert_eq!(wrapped, raw, "OrderedF32 wire not byte-identical to f32");
    }
}
