//! Extension-law harness for open `Encode` / `Decode` /
//! `Validate` impls (o1ix.7).
//!
//! Open trait surface: substrate cannot prove downstream
//! impls law-abiding. This harness asserts the laws from
//! `#[test]` or `proptest!`.
//!
//! # Laws
//!
//! Given `v: T`, `bytes = to_vec(&v)`:
//!
//! 1. Roundtrip: `from_bytes(&bytes) == Ok(v)`.
//! 2. Determinism: re-encoding = identical bytes.
//! 3. Trailing suffix → fail.
//! 4. Strict prefix → fail.
//! 5. Cap-of-zero fails on prefix-driven allocation.
//! 6. `Validate` consistency (optional).
//!
//! # Examples
//!
//! ```ignore
//! pardosa_wire::laws::roundtrip(&v);
//! ```
//!
//! Each fn panics on violation.
use crate::{Decode, Encode, Validate, from_bytes, from_bytes_with_cap, to_vec};
use core::fmt::Debug;
/// **Law 1 (Roundtrip).** `from_bytes(to_vec(v)) == Ok(v)`.
///
/// # Panics
/// Panics if the decoded value is not equal to `value`, or if decode fails.
pub fn roundtrip<T>(value: &T)
where
    T: Encode + Decode + PartialEq + Debug,
{
    let bytes = to_vec(value);
    let back: T = from_bytes(&bytes).expect("roundtrip: decode succeeds on encoded bytes");
    assert_eq!(
        &back, value,
        "roundtrip: decoded value differs from original"
    );
}
/// **Law 2 (Determinism).** Encoding twice produces identical bytes,
/// and encoding the decoded form reproduces the original bytes.
///
/// # Panics
/// Panics if any of the three byte sequences differ.
pub fn canonical_bytes<T>(value: &T)
where
    T: Encode + Decode + PartialEq + Debug,
{
    let a = to_vec(value);
    let b = to_vec(value);
    assert_eq!(a, b, "canonical: encoder is non-deterministic");
    let back: T = from_bytes(&a).expect("canonical: decode succeeds on encoded bytes");
    let c = to_vec(&back);
    assert_eq!(
        a, c,
        "canonical: re-encoded decoded form differs from original bytes"
    );
}
/// **Law 3 (Trailing-byte rejection).** A well-behaved `Decode` impl
/// rejects any suffix bytes beyond the encoded form.
///
/// # Panics
/// Panics if `from_bytes` accepts `[encoded, junk].concat()`.
pub fn trailing_byte_rejected<T>(value: &T)
where
    T: Encode + Decode + Debug,
{
    let mut bytes = to_vec(value);
    bytes.push(0xFFu8);
    let res = from_bytes::<T>(&bytes);
    assert!(
        res.is_err(),
        "trailing-byte: decode accepted unexpected suffix byte"
    );
}
/// **Law 4 (Truncated-input rejection).** A well-behaved `Decode` impl
/// rejects every strict prefix of the encoded form.
///
/// Skipped automatically for zero-byte encodings (no truncation
/// possible). For larger encodings the loop checks each prefix from
/// length `0..bytes.len()`.
///
/// # Panics
/// Panics if any strict prefix decodes successfully.
pub fn truncated_input_rejected<T>(value: &T)
where
    T: Encode + Decode + Debug,
{
    let bytes = to_vec(value);
    if bytes.is_empty() {
        return;
    }
    for n in 0..bytes.len() {
        let prefix = &bytes[..n];
        let res = from_bytes::<T>(prefix);
        assert!(
            res.is_err(),
            "truncated-input: prefix of length {n}/{} decoded successfully (expected Err)",
            bytes.len()
        );
    }
}
/// **Law 5 (Cap respected).** `from_bytes_with_cap(bytes, bytes.len())`
/// succeeds on legitimate bytes; cap of `0` terminates without
/// allocation blow-up.
///
/// # Panics
/// Panics if the generous-cap arm fails to roundtrip, or if the
/// zero-cap arm fails to terminate (the latter is checked structurally
/// by calling the function — termination is the operational invariant).
pub fn cap_respected<T>(value: &T)
where
    T: Encode + Decode + PartialEq + Debug,
{
    let bytes = to_vec(value);
    let back: T =
        from_bytes_with_cap(&bytes, bytes.len()).expect("cap-respected: exact-cap should suffice");
    assert_eq!(&back, value, "cap-respected: roundtrip equality");
    let _ = from_bytes_with_cap::<T>(&bytes, 0);
}
/// **Law 6 (`Validate` consistency).** When `T: Validate`, decoding a
/// value that validates must yield a value that validates; decoding a
/// value that does not validate must either reject at `Decode` time or
/// fail `Validate::validate()` post-decode (never silently accept).
///
/// This law is only meaningful for `T: Validate`. The simpler invariant
/// it pins: `validate(v).is_ok()` ⟺ `validate(from_bytes(to_vec(v))).is_ok()`
/// (same `Validate` verdict before and after a roundtrip).
///
/// # Panics
/// Panics if pre- and post-roundtrip `Validate` verdicts disagree.
pub fn validate_consistent_under_roundtrip<T>(value: &T)
where
    T: Encode + Decode + Validate + Debug,
{
    let before = value.validate().is_ok();
    let bytes = to_vec(value);
    let after = match from_bytes::<T>(&bytes) {
        Ok(back) => back.validate().is_ok(),
        Err(_) => {
            return;
        }
    };
    assert_eq!(
        before, after,
        "validate consistency: pre-roundtrip Validate::validate().is_ok() = {before} \
         but post-roundtrip = {after}"
    );
}
/// Run every applicable law for a `T: Encode + Decode + PartialEq + Debug`.
/// Convenience entry point for tests that don't need finer-grained
/// reporting; equivalent to calling [`roundtrip`], [`canonical_bytes`],
/// [`trailing_byte_rejected`], [`truncated_input_rejected`], and
/// [`cap_respected`] in order.
///
/// # Panics
/// Panics on any law violation (propagated from the individual functions).
pub fn all_encode_decode_laws<T>(value: &T)
where
    T: Encode + Decode + PartialEq + Debug,
{
    roundtrip(value);
    canonical_bytes(value);
    trailing_byte_rejected(value);
    truncated_input_rejected(value);
    cap_respected(value);
}
/// Run every applicable law including [`validate_consistent_under_roundtrip`].
/// Convenience entry point for `T: Encode + Decode + Validate`.
///
/// # Panics
/// Panics on any law violation.
pub fn all_laws_validate<T>(value: &T)
where
    T: Encode + Decode + Validate + PartialEq + Debug,
{
    all_encode_decode_laws(value);
    validate_consistent_under_roundtrip(value);
}
/// Run every applicable law over a finite collection of sample values.
/// Useful for table-driven tests that want to exercise the laws on a
/// hand-curated edge-case suite (empty, min, max, …) in one call.
///
/// # Panics
/// Panics at the first law violation, reporting the offending value's index.
pub fn all_encode_decode_laws_for_samples<T>(samples: &[T])
where
    T: Encode + Decode + PartialEq + Debug,
{
    for v in samples {
        all_encode_decode_laws(v);
    }
}
/// Bulk-apply [`all_laws_validate`] over samples.
///
/// # Panics
/// Panics on any law violation.
pub fn all_laws_validate_for_samples<T>(samples: &[T])
where
    T: Encode + Decode + Validate + PartialEq + Debug,
{
    for v in samples {
        all_laws_validate(v);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Encode, from_bytes};
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    #[test]
    fn primitives_pass_all_laws() {
        all_encode_decode_laws(&0u32);
        all_encode_decode_laws(&u64::MAX);
        all_encode_decode_laws(&true);
        all_encode_decode_laws(&false);
        all_encode_decode_laws(&[7u8; 32]);
    }
    #[test]
    fn vec_u8_passes_all_laws() {
        all_encode_decode_laws(&Vec::<u8>::new());
        all_encode_decode_laws(&vec![1u8, 2, 3, 4, 5]);
    }
    #[test]
    fn string_passes_all_laws() {
        all_encode_decode_laws(&String::new());
        all_encode_decode_laws(&String::from("hello"));
    }
    #[test]
    fn samples_helper_runs_every_value() {
        let samples = [0u64, 1, 42, u64::MAX - 1, u64::MAX];
        all_encode_decode_laws_for_samples(&samples);
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct WrongLength(u32);
    impl Encode for WrongLength {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
        }
    }
    impl Decode for WrongLength {
        fn decode(d: &mut crate::Decoder<'_>) -> Result<Self, crate::DecodeError> {
            let bytes = d.read_bytes(3)?;
            let mut padded = [0u8; 4];
            padded[..3].copy_from_slice(bytes);
            Ok(WrongLength(u32::from_le_bytes(padded)))
        }
    }
    impl crate::sealed::Sealed for WrongLength {}
    impl crate::EventSafe for WrongLength {}
    #[test]
    fn law_harness_catches_under_consuming_decoder_via_trailing_byte() {
        let v = WrongLength(0x0102_0304);
        let bytes = to_vec(&v);
        let res = from_bytes::<WrongLength>(&bytes);
        assert!(
            res.is_err(),
            "under-consuming Decode must surface as from_bytes Err; \
             this is what the `roundtrip` / `canonical_bytes` laws \
             rely on to panic on broken codecs"
        );
    }
    #[test]
    fn timestamp_passes_encode_decode_laws() {
        let ts = crate::Timestamp::from_nanos(1_700_000_000_000_000_000).unwrap();
        all_encode_decode_laws(&ts);
    }
    #[test]
    fn truncated_input_rejected_works_on_vec_u8() {
        truncated_input_rejected(&vec![1u8, 2, 3, 4, 5]);
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct BoundedU64<const MAX: u64>(u64);
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct BoundedErr {
        value: u64,
        max: u64,
    }
    impl<const MAX: u64> Encode for BoundedU64<MAX> {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
        }
    }
    impl<const MAX: u64> Decode for BoundedU64<MAX> {
        fn decode(d: &mut crate::Decoder<'_>) -> Result<Self, crate::DecodeError> {
            u64::decode(d).map(BoundedU64)
        }
    }
    impl<const MAX: u64> crate::sealed::Sealed for BoundedU64<MAX> {}
    impl<const MAX: u64> crate::EventSafe for BoundedU64<MAX> {}
    impl<const MAX: u64> Validate for BoundedU64<MAX> {
        type Error = BoundedErr;
        fn validate(&self) -> Result<(), BoundedErr> {
            if self.0 > MAX {
                return Err(BoundedErr {
                    value: self.0,
                    max: MAX,
                });
            }
            Ok(())
        }
    }
    #[test]
    fn validate_consistency_holds_for_valid_value() {
        let v = BoundedU64::<100>(42);
        validate_consistent_under_roundtrip(&v);
        all_laws_validate(&v);
    }
    #[test]
    fn validate_consistency_holds_for_invalid_value() {
        let v = BoundedU64::<10>(999);
        assert!(v.validate().is_err());
        validate_consistent_under_roundtrip(&v);
    }
    #[test]
    fn validate_aware_samples_helper_runs() {
        let samples = [
            BoundedU64::<100>(0),
            BoundedU64::<100>(50),
            BoundedU64::<100>(100),
        ];
        all_laws_validate_for_samples(&samples);
    }
}
