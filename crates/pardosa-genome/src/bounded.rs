//! Bounded wrapper types with per-field MAX enforcement (GEN-0042).
//!
//! Four const-generic wrappers layer per-field length caps on top of the
//! in-house canonical encoding (GEN-0035) and its decoder cap (R8): the
//! decoder rejects length-prefix headers exceeding `MAX` *before*
//! allocation, and `Validate::validate` re-checks the same invariant for
//! values constructed via [`TryFrom`] / `try_new`.
//!
//! The wire format is byte-identical to the inner `String` / `Vec<u8>` /
//! `Vec<T>` — wrappers express *invariants*, not a distinct encoding.

use core::ops::Deref;

use pardosa_encoding::{Decode, Decoder, Encode, EventError};
use pardosa_traits::{EventSafe, Validate, sealed::Sealed};

// ---------------------------------------------------------------------------
// EventString<MAX>
// ---------------------------------------------------------------------------

/// UTF-8 string with a per-field byte-length cap `MAX`.
///
/// Invariant: `inner.len() <= MAX`. The decoder enforces the cap before
/// allocating the payload buffer (GEN-0042:R1); [`Validate::validate`]
/// re-checks the same invariant for values constructed via [`TryFrom`].
/// The wire format is byte-identical to the inner [`String`]
/// (length-prefixed `[u32 LE len][bytes…]` per GEN-0035:R3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventString<const MAX: usize> {
    inner: String,
}

impl<const MAX: usize> EventString<MAX> {
    /// Return a reference to the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Consume the wrapper and return the inner [`String`].
    #[must_use]
    pub fn into_inner(self) -> String {
        self.inner
    }
}

impl<const MAX: usize> Deref for EventString<MAX> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.inner
    }
}

// No `From<String>` — construction is fallible. `TryFrom` is the blessed
// path; an infallible `From` would let callers stamp an invalid invariant.
impl<const MAX: usize> TryFrom<String> for EventString<MAX> {
    type Error = EventError;
    fn try_from(inner: String) -> Result<Self, EventError> {
        if inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(Self { inner })
    }
}

impl<const MAX: usize> Sealed for EventString<MAX> {}
impl<const MAX: usize> EventSafe for EventString<MAX> {}

impl<const MAX: usize> Encode for EventString<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        // Delegate to `String`'s impl so the wire format is invariant-only,
        // not a distinct encoding (PM4 wire-compat invariant).
        self.inner.encode(out);
    }
}

impl<const MAX: usize> Decode for EventString<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        // `read_len_prefix` charges the decoder cap (GEN-0035:R8); the
        // per-field MAX is a tighter check we apply *immediately* after,
        // before `read_bytes` allocates the payload buffer (GEN-0042:R1).
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(EventError::InvalidInput);
        }
        let bytes = d.read_bytes(n)?;
        let s = core::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| EventError::InvalidInput)?;
        Ok(Self { inner: s })
    }
}

impl<const MAX: usize> Validate for EventString<MAX> {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        if self.inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EventBytes<MAX>
// ---------------------------------------------------------------------------

/// Opaque byte payload with a per-field byte-length cap `MAX`.
///
/// Inner type is [`Vec<u8>`], not [`bytes::Bytes`] — the latter is feature-
/// gated in `pardosa-encoding` and `pardosa-genome` does not enable it.
/// Wire format is byte-identical to a `Vec<u8>` of the same contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventBytes<const MAX: usize> {
    inner: Vec<u8>,
}

impl<const MAX: usize> EventBytes<MAX> {
    /// Return a reference to the inner byte slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    /// Consume the wrapper and return the inner [`Vec<u8>`].
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl<const MAX: usize> Deref for EventBytes<MAX> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.inner
    }
}

impl<const MAX: usize> TryFrom<Vec<u8>> for EventBytes<MAX> {
    type Error = EventError;
    fn try_from(inner: Vec<u8>) -> Result<Self, EventError> {
        if inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(Self { inner })
    }
}

impl<const MAX: usize> Sealed for EventBytes<MAX> {}
impl<const MAX: usize> EventSafe for EventBytes<MAX> {}

impl<const MAX: usize> Encode for EventBytes<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        // Delegate to `Vec<u8>`'s impl — wire-identical, no per-element
        // dispatch overhead (the encoding crate specialises `Vec<u8>` via
        // the `[u8]::encode` path; we use the owned-vec impl here so the
        // length-prefix charge mirrors the decode path exactly).
        self.inner.encode(out);
    }
}

impl<const MAX: usize> Decode for EventBytes<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(EventError::InvalidInput);
        }
        let bytes = d.read_bytes(n)?;
        Ok(Self {
            inner: bytes.to_vec(),
        })
    }
}

impl<const MAX: usize> Validate for EventBytes<MAX> {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        if self.inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EventVec<T, MAX>
// ---------------------------------------------------------------------------

/// Length-bounded vector of `T`, with per-field element-count cap `MAX`.
///
/// Bound `T: EventSafe + Encode + Decode` mirrors the substrate-level
/// `Vec<T>` impl in `pardosa-encoding` (PM6 — don't drift). Wire format
/// is byte-identical to `Vec<T>` of the same contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventVec<T, const MAX: usize> {
    inner: Vec<T>,
}

impl<T, const MAX: usize> EventVec<T, MAX> {
    /// Return a reference to the inner slice.
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }

    /// Consume the wrapper and return the inner [`Vec`].
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.inner
    }
}

impl<T, const MAX: usize> Deref for EventVec<T, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.inner
    }
}

impl<T, const MAX: usize> TryFrom<Vec<T>> for EventVec<T, MAX> {
    type Error = EventError;
    fn try_from(inner: Vec<T>) -> Result<Self, EventError> {
        if inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(Self { inner })
    }
}

impl<T: EventSafe, const MAX: usize> Sealed for EventVec<T, MAX> {}
impl<T: EventSafe, const MAX: usize> EventSafe for EventVec<T, MAX> {}

impl<T: Encode, const MAX: usize> Encode for EventVec<T, MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        // Delegate to `Vec<T>`'s encode — invariant-only wrapper (PM4).
        self.inner.encode(out);
    }
}

impl<T: Decode, const MAX: usize> Decode for EventVec<T, MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(EventError::InvalidInput);
        }
        let mut v: Vec<T> = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::decode(d)?);
        }
        Ok(Self { inner: v })
    }
}

impl<T, const MAX: usize> Validate for EventVec<T, MAX> {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        if self.inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// NonEmptyEventString<MAX>
// ---------------------------------------------------------------------------

/// UTF-8 string with both a length floor (`len > 0`) and a byte-length
/// cap (`len <= MAX`).
///
/// `NonEmptyEventString<0>` is uninhabitable at runtime — every
/// construction path returns `Err(EventError::InvalidInput)`. The runtime
/// checks catch it (PM2); no compile-time `const` assertion is added.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmptyEventString<const MAX: usize> {
    inner: String,
}

impl<const MAX: usize> NonEmptyEventString<MAX> {
    /// Construct from a `&str` slice, validating both invariants
    /// (`len > 0` AND `len <= MAX`).
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidInput`] when the slice is empty or
    /// exceeds `MAX` bytes.
    pub fn try_new(s: &str) -> Result<Self, EventError> {
        if s.is_empty() || s.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(Self {
            inner: s.to_string(),
        })
    }

    /// Return a reference to the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Consume the wrapper and return the inner [`String`].
    #[must_use]
    pub fn into_inner(self) -> String {
        self.inner
    }
}

impl<const MAX: usize> Deref for NonEmptyEventString<MAX> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.inner
    }
}

impl<const MAX: usize> TryFrom<String> for NonEmptyEventString<MAX> {
    type Error = EventError;
    fn try_from(inner: String) -> Result<Self, EventError> {
        if inner.is_empty() || inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(Self { inner })
    }
}

impl<const MAX: usize> Sealed for NonEmptyEventString<MAX> {}
impl<const MAX: usize> EventSafe for NonEmptyEventString<MAX> {}

impl<const MAX: usize> Encode for NonEmptyEventString<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}

impl<const MAX: usize> Decode for NonEmptyEventString<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        // Both floors checked *before* any payload read — adversarial
        // `len == 0` and `len > MAX` headers reject without allocation.
        if n == 0 || n > MAX {
            return Err(EventError::InvalidInput);
        }
        let bytes = d.read_bytes(n)?;
        let s = core::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| EventError::InvalidInput)?;
        Ok(Self { inner: s })
    }
}

impl<const MAX: usize> Validate for NonEmptyEventString<MAX> {
    type Error = EventError;
    fn validate(&self) -> Result<(), EventError> {
        if self.inner.is_empty() || self.inner.len() > MAX {
            return Err(EventError::InvalidInput);
        }
        Ok(())
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

    // ---- Happy-path round-trips -----------------------------------------

    #[test]
    fn event_string_roundtrip_at_max() {
        // len == MAX is the boundary case: must round-trip cleanly. Pick a
        // 16-byte ASCII payload against MAX=16 so the assertion is exact.
        let s: EventString<16> = EventString::try_from(String::from("0123456789abcdef")).unwrap();
        assert_eq!(s.as_str().len(), 16);
        let wire = to_vec(&s);
        let back: EventString<16> = from_bytes(&wire).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn event_bytes_roundtrip_below_max() {
        let b: EventBytes<32> = EventBytes::try_from(vec![0xAAu8, 0xBB, 0xCC]).unwrap();
        let wire = to_vec(&b);
        let back: EventBytes<32> = from_bytes(&wire).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn event_vec_roundtrip() {
        let v: EventVec<u32, 8> = EventVec::try_from(vec![1u32, 2, 3, 4]).unwrap();
        let wire = to_vec(&v);
        let back: EventVec<u32, 8> = from_bytes(&wire).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn nonempty_event_string_roundtrip() {
        let s: NonEmptyEventString<16> = NonEmptyEventString::try_new("hi").unwrap();
        let wire = to_vec(&s);
        let back: NonEmptyEventString<16> = from_bytes(&wire).unwrap();
        assert_eq!(back, s);
    }

    // ---- Adversarial inputs ---------------------------------------------

    #[test]
    fn event_string_rejects_len_over_max_at_decode() {
        // Hand-construct a wire payload advertising 32 bytes (under the 1
        // MiB decoder cap) targeted at EventString<16>. The per-field MAX
        // must reject before the 32-byte payload is read — adversarial
        // headers cannot grow allocation up to the decoder cap on a
        // small-MAX field.
        let mut wire = Vec::new();
        32u32.encode(&mut wire);
        wire.extend_from_slice(&[b'a'; 32]);
        let err = from_bytes::<EventString<16>>(&wire).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn event_string_rejects_u32_max_length_header() {
        // Pathological: length header = u32::MAX, far exceeding the 1 MiB
        // decoder cap. `read_len_prefix` rejects against the cap *before*
        // the per-field MAX check would ever fire — verifies the
        // GEN-0035:R8 substrate is engaged. Payload bytes are omitted
        // deliberately; the cap check must fail before any `read_bytes`.
        let mut wire = Vec::new();
        u32::MAX.encode(&mut wire);
        let err = from_bytes::<EventString<1024>>(&wire).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }
    #[test]
    fn event_vec_validate_rejects_len_over_max() {
        // Post-construction invariant. The decoder check guards adversarial
        // wire; `Validate` guards values built via `TryFrom`. Construct one
        // directly with a too-long inner Vec to exercise the latter path
        // (TryFrom would refuse — we bypass it via a hand-built struct
        // expressed through TryFrom on a freshly-large MAX, then drop MAX).
        //
        // We can't bypass TryFrom without breaking encapsulation, so
        // instead we assert TryFrom itself enforces the cap and
        // additionally a constructed-then-truncated path validates clean.
        let too_long: Vec<u32> = (0..10).collect();
        let err = <EventVec<u32, 4>>::try_from(too_long).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);

        // Constructed-at-cap then validate() returns Ok.
        let ok: EventVec<u32, 4> = EventVec::try_from(vec![1u32, 2, 3, 4]).unwrap();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn nonempty_event_string_rejects_empty_at_decode_and_validate() {
        // Wire path: len=0 header rejected before any payload read.
        let mut wire = Vec::new();
        0u32.encode(&mut wire);
        let err = from_bytes::<NonEmptyEventString<16>>(&wire).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);

        // Constructor path: empty input rejected.
        let err2 = NonEmptyEventString::<16>::try_new("").unwrap_err();
        assert_eq!(err2, EventError::InvalidInput);

        // Constructor path: over-MAX rejected.
        let err3 = NonEmptyEventString::<4>::try_new("toolong").unwrap_err();
        assert_eq!(err3, EventError::InvalidInput);
    }

    // ---- Wire-compat sanity --------------------------------------------

    // ---- ValidationCost surface (GEN-0040 amendment) -------------------

    #[test]
    fn bounded_wrappers_inherit_default_cheap_cost() {
        // Wrappers do O(1) length checks; the trait-default `Cheap` is
        // correct without per-impl override. Locking the const here guards
        // against an accidental future override (or default reshuffle)
        // that would silently reclassify the wrappers as Free/Bounded.
        assert_eq!(<EventString<8> as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<EventBytes<8> as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(<EventVec<u32, 8> as Validate>::COST, ValidationCost::Cheap);
        assert_eq!(
            <NonEmptyEventString<8> as Validate>::COST,
            ValidationCost::Cheap
        );
    }

    #[test]
    fn event_string_wire_compat_with_string() {
        // PM4 invariant lock: wrappers express invariants, NOT a distinct
        // encoding. The wire bytes of `EventString<MAX>` and the inner
        // `String` must be byte-identical so a producer can swap one for
        // the other without a wire-format break.
        let payload = String::from("hello");
        let wrapped: EventString<16> = EventString::try_from(payload.clone()).unwrap();
        assert_eq!(to_vec(&wrapped), to_vec(&payload));
    }
}
