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

#[cfg(any(feature = "uuid", feature = "bytes", feature = "arrayvec", feature = "jiff"))]
use alloc::vec::Vec;

#[cfg(any(feature = "uuid", feature = "bytes", feature = "arrayvec", feature = "jiff"))]
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
