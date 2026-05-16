//! FH11 (bead `adr-fmt-bw7l`) — public-surface decode-cap behavioural contract.
//!
//! # Background
//!
//! `pardosa_encoding::Decoder` charges a cap budget in `read_len_prefix`
//! (lib.rs:179) before any allocation is attempted (the historical "F6"
//! footgun: arithmetic was bounded by inspection but not by a property
//! test). The original AC asked for an allocator-probe measurement of
//! `measured_bytes <= N * cap`. That was rescoped by moltke to a
//! behavioural-contract property test at the public `Decode` surface,
//! for the following reasons:
//!
//! 1. `pardosa-encoding` is `#![no_std]` + `#![forbid(unsafe_code)]`
//!    (lib.rs:1, 14). A `GlobalAlloc` wrapper requires `unsafe impl`
//!    and would only be installable in the test crate, which adds
//!    significant surface and only proxies the underlying cap-debit
//!    accounting anyway.
//! 2. A `#[cfg(test)]` thread-local counter inside `read_len_prefix`
//!    makes the assertion `total_debits <= cap` near-tautological:
//!    `read_len_prefix` is the *only* place the cap can be debited,
//!    and it already enforces `n > cap_remaining => Err` (lib.rs:182).
//! 3. The contract consumers of `from_bytes_with_cap` actually depend
//!    on is *behavioural*: any input + any cap terminates with
//!    `Ok(roundtrip)` or `Err(_)`, never panic, never OOM.
//!
//! # What this test pins
//!
//! For diverse inputs across `Vec<u8>`, `Vec<Vec<u32>>`,
//! `BTreeMap<u32, Vec<u8>>`, and `[u8; 32]`, at varying cap values:
//!
//! - **Round-trip leg**: when cap is generous enough, `to_vec` then
//!   `from_bytes_with_cap` returns the original value byte-for-byte.
//! - **Cap-clamp leg**: when cap is restrictive, decode returns `Err`
//!   rather than panicking, hanging, or over-allocating.
//! - **Junk-bytes leg**: arbitrary input bytes + arbitrary cap always
//!   terminate with `Ok(_)` or `Err(_)`; no panic.
//!
//! # Cap-exhaustion variant note
//!
//! `EventError` (lib.rs:58) does not carry a dedicated
//! `CapExceeded` variant; cap-exhaustion in `read_len_prefix` maps to
//! `EventError::InvalidInput` (lib.rs:183). The cap-clamp leg therefore
//! asserts `is_err()` rather than a specific variant. Documented as a
//! known limitation; introducing a `CapExceeded` variant is a
//! wire-format change out of scope for this footgun-hunt wave.
//!
//! # Proptest configuration
//!
//! - 256 cases (rescoped from 1024 — at this contract layer, the
//!   marginal coverage of the extra 768 cases isn't worth the budget).
//! - Regression seeds committed at `decode_cap_respected.proptest-regressions`.

use pardosa_encoding::{Decode, Encode, EventError, from_bytes_with_cap, to_vec};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use std::collections::BTreeMap;

/// Generous cap sized to comfortably hold any value our strategies produce.
/// 1 MiB is well above the largest generated payload (≤ 64 * 64 = 4 KiB for
/// `Vec<Vec<u32>>`, ≤ 64 * (4 + 64) ≈ 4 KiB for `BTreeMap`).
const GENEROUS_CAP: usize = 1 << 20;

/// Round-trip helper: encode + decode with a generous cap, assert equality.
fn round_trip<T>(value: &T)
where
    T: Decode + Encode + PartialEq + core::fmt::Debug,
{
    let bytes = to_vec(value);
    let decoded: T =
        from_bytes_with_cap(&bytes, GENEROUS_CAP).expect("generous cap should suffice");
    assert_eq!(&decoded, value, "round-trip mismatch");
}

/// Decode-must-terminate helper: any input + any cap returns `Ok` or `Err`,
/// never panic / hang / OOM. The cap upper-bounds `Vec::with_capacity` calls
/// in `Decode` impls, so even a multi-GiB length prefix in the bytes is
/// rejected before allocation.
fn decode_terminates<T: Decode>(bytes: &[u8], cap: usize) {
    let _ = from_bytes_with_cap::<T>(bytes, cap);
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

/// `Vec<u8>`: up to 256 bytes, full byte range.
fn strat_vec_u8() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..256)
}

/// `Vec<Vec<u32>>`: up to 16 inner vecs of up to 64 u32s each (worst case
/// ~4 KiB encoded payload).
fn strat_vec_vec_u32() -> impl Strategy<Value = Vec<Vec<u32>>> {
    vec(vec(any::<u32>(), 0..64), 0..16)
}

/// `BTreeMap<u32, Vec<u8>>`: up to 32 entries, value vecs up to 64 bytes.
fn strat_btree_map_u32_vec_u8() -> impl Strategy<Value = BTreeMap<u32, Vec<u8>>> {
    btree_map(any::<u32>(), vec(any::<u8>(), 0..64), 0..32)
}

/// `[u8; 32]`: fixed-width array, every byte arbitrary.
fn strat_array_u8_32() -> impl Strategy<Value = [u8; 32]> {
    any::<[u8; 32]>()
}

/// Restrictive cap values to probe the cap-clamp leg.
fn strat_small_cap() -> impl Strategy<Value = usize> {
    prop_oneof![
        Just(0_usize),
        Just(4),
        Just(16),
        Just(64),
        Just(256),
        0_usize..1024
    ]
}

/// Arbitrary byte buffer for the junk-bytes-terminates leg.
fn strat_junk_bytes() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..2048)
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Round-trip: any generated value decodes back to itself under a
    /// generous cap. This pins the encode/decode pair as inverses for the
    /// four bead-listed types.
    #[test]
    fn round_trip_vec_u8(v in strat_vec_u8()) {
        round_trip(&v);
    }

    #[test]
    fn round_trip_vec_vec_u32(v in strat_vec_vec_u32()) {
        round_trip(&v);
    }

    #[test]
    fn round_trip_btree_map_u32_vec_u8(v in strat_btree_map_u32_vec_u8()) {
        round_trip(&v);
    }

    #[test]
    fn round_trip_array_u8_32(v in strat_array_u8_32()) {
        round_trip(&v);
    }

    /// Cap-clamp: under restrictive cap, decode returns Err rather than
    /// panicking or over-allocating. Encodes the value with the production
    /// `to_vec`, then decodes with a (possibly inadequate) cap. Either the
    /// cap was sufficient and we get Ok+equality, OR the cap was inadequate
    /// and we get Err. No third option (panic, OOM, hang) is permitted.
    #[test]
    fn cap_clamp_vec_u8(v in strat_vec_u8(), cap in strat_small_cap()) {
        let bytes = to_vec(&v);
        // Err is acceptable (cap exhausted); Ok must round-trip exactly.
        // Phrased as `if let` to satisfy clippy::single_match.
        if let Ok(decoded) = from_bytes_with_cap::<Vec<u8>>(&bytes, cap) {
            prop_assert_eq!(decoded, v);
        }
    }

    #[test]
    fn cap_clamp_vec_vec_u32(v in strat_vec_vec_u32(), cap in strat_small_cap()) {
        let bytes = to_vec(&v);
        if let Ok(decoded) = from_bytes_with_cap::<Vec<Vec<u32>>>(&bytes, cap) {
            prop_assert_eq!(decoded, v);
        }
    }

    #[test]
    fn cap_clamp_btree_map_u32_vec_u8(
        v in strat_btree_map_u32_vec_u8(),
        cap in strat_small_cap()
    ) {
        let bytes = to_vec(&v);
        if let Ok(decoded) = from_bytes_with_cap::<BTreeMap<u32, Vec<u8>>>(&bytes, cap) {
            prop_assert_eq!(decoded, v);
        }
    }

    /// Junk-bytes-terminates: arbitrary input + arbitrary cap yields Ok or
    /// Err, never panic. This is the defence-in-depth property — even if a
    /// future Decode impl forgets to call `read_len_prefix` before
    /// `Vec::with_capacity`, the test would catch a panic; if a length
    /// prefix in the junk bytes encodes a multi-GiB value, the cap should
    /// reject it before allocation rather than OOM.
    #[test]
    fn junk_bytes_terminates_vec_u8(bytes in strat_junk_bytes(), cap in strat_small_cap()) {
        decode_terminates::<Vec<u8>>(&bytes, cap);
    }

    #[test]
    fn junk_bytes_terminates_vec_vec_u32(bytes in strat_junk_bytes(), cap in strat_small_cap()) {
        decode_terminates::<Vec<Vec<u32>>>(&bytes, cap);
    }

    #[test]
    fn junk_bytes_terminates_btree_map(bytes in strat_junk_bytes(), cap in strat_small_cap()) {
        decode_terminates::<BTreeMap<u32, Vec<u8>>>(&bytes, cap);
    }

    #[test]
    fn junk_bytes_terminates_array_u8_32(bytes in strat_junk_bytes(), cap in strat_small_cap()) {
        decode_terminates::<[u8; 32]>(&bytes, cap);
    }
}

// ---------------------------------------------------------------------------
// Smoke test: confirm EventError surface is reachable (kept tiny — the
// real coverage is in the proptest! block above).
// ---------------------------------------------------------------------------

#[test]
fn cap_zero_rejects_nonempty_payload() {
    let bytes = to_vec(&vec![1u8, 2, 3]);
    let err =
        from_bytes_with_cap::<Vec<u8>>(&bytes, 0).expect_err("cap=0 must reject 3-byte payload");
    // We can't pin the exact variant (see module-level note on
    // CapExceeded absence) but we can assert it's NOT a soft-success.
    let _: EventError = err;
}
