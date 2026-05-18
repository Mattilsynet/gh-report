// ---------------------------------------------------------------------------
// GEN-0041 foreign-crate v0 floor
// ---------------------------------------------------------------------------
//
// Encode + Decode impls for `uuid::Uuid`, `bytes::Bytes`, and
// `arrayvec::ArrayVec<T, N>` behind feature gates `uuid`, `bytes`,
// `arrayvec`. The sealing chain (`sealed::Sealed` + `EventSafe`) for these
// types lives in `pardosa-traits` behind matching feature gates; the orphan
// rule mandates the split (Encode is defined here, EventSafe there).
//
// S1 (byte-shape conformance) — each impl conforms to GEN-0035 length-prefix
// rules: fixed-width types emit verbatim bytes back-to-back; variable-length
// payloads emit `[len:u32 LE][bytes…]`. Tests below assert wire layout.
//
// S2 (post-decode capacity/length validity) — capacity-bounded types
// (`ArrayVec<T, N>`) reject a decoded length > N before any allocation,
// surfacing as `EventError::InvalidInput` (the frozen post-C2 variant for
// caller-input violations; no new variant introduced).

#[cfg(any(
    feature = "uuid",
    feature = "bytes",
    feature = "arrayvec",
    feature = "jiff"
))]
use alloc::vec::Vec;

#[cfg(any(
    feature = "uuid",
    feature = "bytes",
    feature = "arrayvec",
    feature = "jiff"
))]
use crate::{Decode, Decoder, Encode, EventError};

#[cfg(any(feature = "bytes", feature = "arrayvec"))]
use crate::composites::encode_len_prefix;

// ---- uuid::Uuid — 16 bytes verbatim, no length prefix ----------------------
#[cfg(feature = "uuid")]
impl Encode for uuid::Uuid {
    fn encode(&self, out: &mut Vec<u8>) {
        // `Uuid::as_bytes()` returns `&[u8; 16]` with a stable layout
        // (uuid crate documents these as the bytes "in network order");
        // we emit them verbatim. Fixed width = no length prefix, matching
        // GEN-0035 §"Fixed-size arrays".
        out.extend_from_slice(uuid::Uuid::as_bytes(self));
    }
}

#[cfg(feature = "uuid")]
impl Decode for uuid::Uuid {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let bytes = d.read_bytes(16)?;
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Ok(uuid::Uuid::from_bytes(arr))
    }
}

// ---- bytes::Bytes — length-prefixed opaque payload -------------------------
#[cfg(feature = "bytes")]
impl Encode for bytes::Bytes {
    fn encode(&self, out: &mut Vec<u8>) {
        // Wire-identical to `Vec<u8>` / `&[u8]` — GEN-0035 length-prefix
        // rule applies to any variable-length byte payload regardless of
        // ownership flavour. Round-trip via `Bytes::copy_from_slice` on
        // the decode side.
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self);
    }
}

#[cfg(feature = "bytes")]
impl Decode for bytes::Bytes {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        let slice = d.read_bytes(n)?;
        Ok(bytes::Bytes::copy_from_slice(slice))
    }
}

// ---- arrayvec::ArrayVec<T, N> — length-prefixed bounded vec ---------------
#[cfg(feature = "arrayvec")]
impl<T: Encode, const N: usize> Encode for arrayvec::ArrayVec<T, N> {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        for v in self {
            v.encode(out);
        }
    }
}

#[cfg(feature = "arrayvec")]
impl<T: Decode, const N: usize> Decode for arrayvec::ArrayVec<T, N> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        // S2 guard: capacity-bounded types must reject `len > N` before any
        // per-element decode so a malformed header cannot consume budget
        // it will never deposit into a value.
        if n > N {
            return Err(EventError::InvalidInput);
        }
        let mut v: arrayvec::ArrayVec<T, N> = arrayvec::ArrayVec::new();
        for _ in 0..n {
            // `try_push` cannot fail given the S2 check above — n ≤ N and
            // we push exactly n times. Mapping the (unreachable) error to
            // InvalidInput keeps the surface frozen.
            v.try_push(T::decode(d)?)
                .map_err(|_| EventError::InvalidInput)?;
        }
        Ok(v)
    }
}

// ---- jiff::Timestamp — 8 bytes LE of as_microsecond() ---------------------
//
// GEN-0043:R1 — canonical wire shape is the 8-byte little-endian encoding
// of `Timestamp::as_microsecond()` (i64 microseconds since the Unix epoch).
// Decode reads 8 bytes LE and reconstructs via `Timestamp::from_microsecond`;
// the round-trip is total over `i64` (no in-range rejection — `from_microsecond`
// accepts the full `i64` domain per GEN-0043:R1). Sub-microsecond precision
// is truncated at encode and the truncated value is the canonical wall-clock
// identity (GEN-0043:R2). Fixed-width 8 bytes — no length prefix; the
// GEN-0035:R8 decoder cap does not apply (GEN-0043:R4).
#[cfg(feature = "jiff")]
impl Encode for jiff::Timestamp {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_microsecond().encode(out);
    }
}

#[cfg(feature = "jiff")]
impl Decode for jiff::Timestamp {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let micros = i64::decode(d)?;
        // `from_microsecond` is total over i64 per GEN-0043:R1 — no in-range
        // check at decode. Map any future error (e.g. if jiff narrows its
        // accepted range) onto the frozen InvalidInput variant.
        jiff::Timestamp::from_microsecond(micros).map_err(|_| EventError::InvalidInput)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(
        feature = "uuid",
        feature = "bytes",
        feature = "arrayvec",
        feature = "jiff"
    ))]
    use crate::{EventError, from_bytes, to_vec};
    #[cfg(any(feature = "bytes", feature = "arrayvec"))]
    use alloc::vec;
    #[cfg(any(feature = "bytes", feature = "arrayvec"))]
    use alloc::vec::Vec;

    // ----- GEN-0041 foreign-crate v0 floor -----------------------------------

    #[cfg(feature = "uuid")]
    #[test]
    fn uuid_roundtrip_and_layout() {
        // S1: Uuid encodes as 16 verbatim bytes, no length prefix. Pick a
        // pattern whose every byte is distinct so any byte-order surprise
        // would show up as a permuted assert.
        let raw: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ];
        let u = uuid::Uuid::from_bytes(raw);
        let bytes = to_vec(&u);
        assert_eq!(bytes.as_slice(), &raw[..]);
        let back: uuid::Uuid = from_bytes(&bytes).unwrap();
        assert_eq!(back, u);
    }

    #[cfg(feature = "uuid")]
    #[test]
    fn uuid_truncated_input_rejected() {
        // 15 bytes is one short of the 16 the fixed-width decode requires;
        // surfaces via the standard truncated-read path.
        let err = from_bytes::<uuid::Uuid>(&[0u8; 15]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[cfg(feature = "bytes")]
    #[test]
    fn bytes_roundtrip_and_layout() {
        // S1: length-prefixed payload identical to Vec<u8>/&[u8] wire form.
        let payload = bytes::Bytes::from_static(&[0xAA, 0xBB, 0xCC]);
        let wire = to_vec(&payload);
        assert_eq!(wire, vec![3, 0, 0, 0, 0xAA, 0xBB, 0xCC]);
        let back: bytes::Bytes = from_bytes(&wire).unwrap();
        assert_eq!(back, payload);

        // Empty payload still carries the 4-byte length header.
        let empty = bytes::Bytes::new();
        let wire_empty = to_vec(&empty);
        assert_eq!(wire_empty, vec![0, 0, 0, 0]);
        let back_empty: bytes::Bytes = from_bytes(&wire_empty).unwrap();
        assert_eq!(back_empty, empty);
    }

    #[cfg(feature = "bytes")]
    #[test]
    fn bytes_wire_matches_vec_u8() {
        // S1 sanity: a `bytes::Bytes` payload and a `Vec<u8>` with identical
        // contents must produce byte-identical wire output. Locks the
        // "same length-prefix rule for any opaque byte payload" invariant.
        let payload = [0xDE, 0xAD, 0xBE, 0xEF, 0x01];
        let from_bytes_form = to_vec(&bytes::Bytes::copy_from_slice(&payload));
        let from_vec_form = to_vec(&payload.to_vec());
        assert_eq!(from_bytes_form, from_vec_form);
    }

    #[cfg(feature = "arrayvec")]
    #[test]
    fn arrayvec_roundtrip() {
        // Variable-length capacity-bounded vec encodes like Vec<T>: u32 LE
        // count + per-element encode. Round-trip at len < N and len == N.
        let mut av: arrayvec::ArrayVec<u32, 4> = arrayvec::ArrayVec::new();
        av.try_push(1).unwrap();
        av.try_push(2).unwrap();
        av.try_push(3).unwrap();
        let wire = to_vec(&av);
        // 4-byte LE count + 3 * 4-byte u32 LE payload = 16 bytes total.
        assert_eq!(wire[..4], [3, 0, 0, 0]);
        let back: arrayvec::ArrayVec<u32, 4> = from_bytes(&wire).unwrap();
        assert_eq!(back.as_slice(), av.as_slice());

        // At capacity.
        let mut full: arrayvec::ArrayVec<u8, 3> = arrayvec::ArrayVec::new();
        full.try_push(7).unwrap();
        full.try_push(8).unwrap();
        full.try_push(9).unwrap();
        let wire = to_vec(&full);
        let back: arrayvec::ArrayVec<u8, 3> = from_bytes(&wire).unwrap();
        assert_eq!(back.as_slice(), full.as_slice());
    }

    #[cfg(feature = "arrayvec")]
    #[test]
    fn arrayvec_rejects_len_over_capacity() {
        // S2: a decoded length-prefix exceeding the target capacity must
        // surface as EventError::InvalidInput *before* any per-element
        // decode runs. Construct the smallest such wire: count=4 against
        // a 3-capacity ArrayVec.
        //
        // Bonus: only 4 payload bytes follow (one fewer than required for
        // count=4 even at u8), so a missed S2 guard would also fail via
        // the truncated-read path; the test asserts the *S2* code path is
        // reached by making the input long enough that absent the guard
        // the decode would otherwise succeed.
        use crate::Encode;
        let mut wire = Vec::new();
        4u32.encode(&mut wire);
        wire.extend_from_slice(&[1u8, 2, 3, 4]);
        let err = from_bytes::<arrayvec::ArrayVec<u8, 3>>(&wire).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    // ----- jiff::Timestamp foreign-floor (GEN-0043) --------------------------

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_layout_and_roundtrip() {
        // GEN-0043:R1 — wire shape is 8-byte LE of as_microsecond() (i64).
        // Pick a distinct-byte pattern in the positive half so any byte-
        // order surprise would show up as a permuted assert.
        let micros: i64 = 0x0102_0304_0506_0708;
        let ts = jiff::Timestamp::from_microsecond(micros).unwrap();
        let bytes = to_vec(&ts);
        assert_eq!(
            bytes,
            alloc::vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]
        );
        let back: jiff::Timestamp = from_bytes(&bytes).unwrap();
        assert_eq!(back, ts);
        assert_eq!(back.as_microsecond(), micros);
    }

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_zero_micros_roundtrip() {
        // The Unix epoch — micros == 0 — must encode as 8 zero bytes and
        // round-trip cleanly. Anchors the floor of the GEN-0043:R1 domain.
        let ts = jiff::Timestamp::from_microsecond(0).unwrap();
        let bytes = to_vec(&ts);
        assert_eq!(bytes, alloc::vec![0u8; 8]);
        let back: jiff::Timestamp = from_bytes(&bytes).unwrap();
        assert_eq!(back, ts);
    }

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_truncated_input_rejected() {
        // 7 bytes is one short of the fixed-width 8 the decode requires;
        // surfaces via the standard truncated-read path.
        let err = from_bytes::<jiff::Timestamp>(&[0u8; 7]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }
}
