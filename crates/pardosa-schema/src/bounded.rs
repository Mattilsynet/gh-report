use crate::error::DomainError;
use crate::genome_safe::{GenomeSafe, schema_hash_bytes, schema_hash_combine};
use core::ops::Deref;
use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
use pardosa_wire::{EventSafe, Validate};
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventString<const MAX: usize> {
    inner: String,
}
impl<const MAX: usize> EventString<MAX> {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }
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
impl<const MAX: usize> TryFrom<String> for EventString<MAX> {
    type Error = DomainError;
    fn try_from(inner: String) -> Result<Self, DomainError> {
        if inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: inner.len(),
            });
        }
        Ok(Self { inner })
    }
}
impl<const MAX: usize> pardosa_wire::sealed::Sealed for EventString<MAX> {}
impl<const MAX: usize> EventSafe for EventString<MAX> {}
impl<const MAX: usize> Encode for EventString<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl<const MAX: usize> Decode for EventString<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: n,
            }
            .into());
        }
        let bytes = d.read_bytes(n)?;
        let s = core::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| DecodeError::from(DomainError::InvalidUtf8))?;
        Ok(Self { inner: s })
    }
}
impl<const MAX: usize> Validate for EventString<MAX> {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        if self.inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: self.inner.len(),
            });
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventBytes<const MAX: usize> {
    inner: Vec<u8>,
}
impl<const MAX: usize> EventBytes<MAX> {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }
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
    type Error = DomainError;
    fn try_from(inner: Vec<u8>) -> Result<Self, DomainError> {
        if inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: inner.len(),
            });
        }
        Ok(Self { inner })
    }
}
impl<const MAX: usize> pardosa_wire::sealed::Sealed for EventBytes<MAX> {}
impl<const MAX: usize> EventSafe for EventBytes<MAX> {}
impl<const MAX: usize> Encode for EventBytes<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl<const MAX: usize> Decode for EventBytes<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: n,
            }
            .into());
        }
        let bytes = d.read_bytes(n)?;
        Ok(Self {
            inner: bytes.to_vec(),
        })
    }
}
impl<const MAX: usize> Validate for EventBytes<MAX> {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        if self.inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: self.inner.len(),
            });
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventVec<T, const MAX: usize> {
    inner: Vec<T>,
}
impl<T, const MAX: usize> EventVec<T, MAX> {
    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }
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
    type Error = DomainError;
    fn try_from(inner: Vec<T>) -> Result<Self, DomainError> {
        if inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: inner.len(),
            });
        }
        Ok(Self { inner })
    }
}
impl<T: GenomeSafe, const MAX: usize> pardosa_wire::sealed::Sealed for EventVec<T, MAX> {}
impl<T: GenomeSafe, const MAX: usize> EventSafe for EventVec<T, MAX> {}
impl<T: Encode, const MAX: usize> Encode for EventVec<T, MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl<T: Decode, const MAX: usize> Decode for EventVec<T, MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        if n > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: n,
            }
            .into());
        }
        let mut v: Vec<T> = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::decode(d)?);
        }
        Ok(Self { inner: v })
    }
}
impl<T, const MAX: usize> Validate for EventVec<T, MAX> {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        if self.inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: self.inner.len(),
            });
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NonEmptyEventString<const MAX: usize> {
    inner: String,
}
impl<const MAX: usize> NonEmptyEventString<MAX> {
    /// Build a `NonEmptyEventString` from a `&str`, enforcing non-emptiness and `MAX`.
    ///
    /// # Errors
    /// Returns `DomainError::Empty` if `s` is empty, or `DomainError::TooLong` if `s.len() > MAX`.
    pub fn try_new(s: &str) -> Result<Self, DomainError> {
        if s.is_empty() {
            return Err(DomainError::Empty);
        }
        if s.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: s.len(),
            });
        }
        Ok(Self {
            inner: s.to_string(),
        })
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.inner
    }
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
    type Error = DomainError;
    fn try_from(inner: String) -> Result<Self, DomainError> {
        if inner.is_empty() {
            return Err(DomainError::Empty);
        }
        if inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: inner.len(),
            });
        }
        Ok(Self { inner })
    }
}
impl<const MAX: usize> pardosa_wire::sealed::Sealed for NonEmptyEventString<MAX> {}
impl<const MAX: usize> EventSafe for NonEmptyEventString<MAX> {}
impl<const MAX: usize> Encode for NonEmptyEventString<MAX> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.inner.encode(out);
    }
}
impl<const MAX: usize> Decode for NonEmptyEventString<MAX> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        if n == 0 {
            return Err(DomainError::Empty.into());
        }
        if n > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: n,
            }
            .into());
        }
        let bytes = d.read_bytes(n)?;
        let s = core::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| DecodeError::from(DomainError::InvalidUtf8))?;
        Ok(Self { inner: s })
    }
}
impl<const MAX: usize> Validate for NonEmptyEventString<MAX> {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        if self.inner.is_empty() {
            return Err(DomainError::Empty);
        }
        if self.inner.len() > MAX {
            return Err(DomainError::TooLong {
                max: MAX,
                actual: self.inner.len(),
            });
        }
        Ok(())
    }
}
impl<const MAX: usize> GenomeSafe for EventString<MAX> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"EventString"), MAX as u128);
    const SCHEMA_SOURCE: &'static str = "EventString<MAX>";
}
impl<const MAX: usize> GenomeSafe for NonEmptyEventString<MAX> {
    const SCHEMA_HASH: u128 =
        schema_hash_combine(schema_hash_bytes(b"NonEmptyEventString"), MAX as u128);
    const SCHEMA_SOURCE: &'static str = "NonEmptyEventString<MAX>";
}
impl<const MAX: usize> GenomeSafe for EventBytes<MAX> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"EventBytes"), MAX as u128);
    const SCHEMA_SOURCE: &'static str = "EventBytes<MAX>";
}
impl<T: GenomeSafe, const MAX: usize> GenomeSafe for EventVec<T, MAX> {
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_bytes(b"EventVec"),
        schema_hash_combine(T::SCHEMA_HASH, MAX as u128),
    );
    const SCHEMA_SOURCE: &'static str = "EventVec<T, MAX>";
}
#[cfg(test)]
mod tests {
    use super::*;
    use pardosa_wire::DecodeError;
    use pardosa_wire::ValidationCost;
    use pardosa_wire::{from_bytes, to_vec};
    #[test]
    fn event_string_roundtrip_at_max() {
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
    #[test]
    fn event_string_rejects_len_over_max_at_decode() {
        let mut wire = Vec::new();
        32u32.encode(&mut wire);
        wire.extend_from_slice(&[b'a'; 32]);
        let err = from_bytes::<EventString<16>>(&wire).unwrap_err();
        assert_eq!(
            err,
            DecodeError::SchemaRejected {
                code: pardosa_wire::SchemaRejectionCode::TooLong
            }
        );
    }
    #[test]
    fn event_string_rejects_u32_max_length_header() {
        let mut wire = Vec::new();
        u32::MAX.encode(&mut wire);
        let err = from_bytes::<EventString<1024>>(&wire).unwrap_err();
        assert!(matches!(err, DecodeError::LengthOutOfRange { .. }));
    }
    #[test]
    fn event_vec_validate_rejects_len_over_max() {
        let too_long: Vec<u32> = (0..10).collect();
        let err = <EventVec<u32, 4>>::try_from(too_long).unwrap_err();
        assert!(matches!(err, DomainError::TooLong { max: 4, actual: 10 }));
        let ok: EventVec<u32, 4> = EventVec::try_from(vec![1u32, 2, 3, 4]).unwrap();
        assert!(ok.validate().is_ok());
    }
    #[test]
    fn nonempty_event_string_rejects_empty_at_decode_and_validate() {
        let mut wire = Vec::new();
        0u32.encode(&mut wire);
        let err = from_bytes::<NonEmptyEventString<16>>(&wire).unwrap_err();
        assert_eq!(
            err,
            DecodeError::SchemaRejected {
                code: pardosa_wire::SchemaRejectionCode::Empty
            }
        );
        let err2 = NonEmptyEventString::<16>::try_new("").unwrap_err();
        assert_eq!(err2, DomainError::Empty);
        let err3 = NonEmptyEventString::<4>::try_new("toolong").unwrap_err();
        assert!(matches!(err3, DomainError::TooLong { max: 4, actual: 7 }));
    }
    #[test]
    fn bounded_wrappers_inherit_default_cheap_cost() {
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
        let payload = String::from("hello");
        let wrapped: EventString<16> = EventString::try_from(payload.clone()).unwrap();
        assert_eq!(to_vec(&wrapped), to_vec(&payload));
    }
    #[test]
    fn bounded_wrappers_have_genome_safe_schema_hash() {
        let _ = <EventString<256> as GenomeSafe>::SCHEMA_HASH;
        let _ = <EventBytes<256> as GenomeSafe>::SCHEMA_HASH;
        let _ = <NonEmptyEventString<256> as GenomeSafe>::SCHEMA_HASH;
        let _ = <EventVec<u32, 16> as GenomeSafe>::SCHEMA_HASH;
    }
    #[test]
    fn event_string_schema_hash_differs_across_max() {
        assert_ne!(
            <EventString<128> as GenomeSafe>::SCHEMA_HASH,
            <EventString<256> as GenomeSafe>::SCHEMA_HASH,
        );
    }
    #[test]
    fn event_vec_schema_hash_composes_inner_and_max() {
        assert_ne!(
            <EventVec<u32, 16> as GenomeSafe>::SCHEMA_HASH,
            <EventVec<u64, 16> as GenomeSafe>::SCHEMA_HASH,
        );
        assert_ne!(
            <EventVec<u32, 16> as GenomeSafe>::SCHEMA_HASH,
            <EventVec<u32, 32> as GenomeSafe>::SCHEMA_HASH,
        );
    }
    #[test]
    fn bounded_wrapper_schema_sources_are_human_readable() {
        assert_eq!(
            <EventString<256> as GenomeSafe>::SCHEMA_SOURCE,
            "EventString<MAX>"
        );
        assert_eq!(
            <EventBytes<256> as GenomeSafe>::SCHEMA_SOURCE,
            "EventBytes<MAX>"
        );
        assert_eq!(
            <NonEmptyEventString<256> as GenomeSafe>::SCHEMA_SOURCE,
            "NonEmptyEventString<MAX>",
        );
        assert_eq!(
            <EventVec<u32, 16> as GenomeSafe>::SCHEMA_SOURCE,
            "EventVec<T, MAX>"
        );
    }
}
