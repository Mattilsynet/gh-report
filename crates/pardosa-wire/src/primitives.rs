use crate::{Decode, DecodeError, Decoder, Encode};
use alloc::vec::Vec;
impl Encode for u8 {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }
}
impl Decode for u8 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(d.read_bytes(1)?[0])
    }
}
impl Encode for i8 {
    fn encode(&self, out: &mut Vec<u8>) {
        #[allow(
            clippy::cast_sign_loss,
            reason = "non-idiomatic Rust required: wire-protocol i8↔u8 reinterpretation per GEN-0035 is by-design bit reuse; `try_from` would reject negative values that are valid on the wire"
        )]
        out.push(*self as u8);
    }
}
impl Decode for i8 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        #[allow(
            clippy::cast_possible_wrap,
            reason = "non-idiomatic Rust required: wire-protocol u8↔i8 reinterpretation per GEN-0035 is by-design bit reuse; `try_from` would reject high-bit-set bytes that are valid on the wire"
        )]
        Ok(d.read_bytes(1)?[0] as i8)
    }
}
macro_rules! impl_le_primitive {
    ($($ty:ty => $n:expr),+ $(,)?) => {
        $(impl Encode for $ty { fn encode(& self, out : & mut Vec < u8 >) { out
        .extend_from_slice(& self.to_le_bytes()); } } impl Decode for $ty { fn decode(d :
        & mut Decoder <'_ >) -> Result < Self, DecodeError > { let bytes = d
        .read_bytes($n) ?; let mut arr = [0u8; $n]; arr.copy_from_slice(bytes); Ok(<$ty
        >::from_le_bytes(arr)) } })+
    };
}
impl_le_primitive!(
    u16 => 2, u32 => 4, u64 => 8, u128 => 16, i16 => 2, i32 => 4, i64 => 8, i128 => 16,
    f32 => 4, f64 => 8,
);
impl Encode for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}
impl Decode for bool {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match d.read_bytes(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::TagOutOfRange {
                tag: u32::from(tag),
            }),
        }
    }
}
impl Encode for () {
    fn encode(&self, _out: &mut Vec<u8>) {}
}
impl Decode for () {
    fn decode(_d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use crate::{Decode, DecodeError, Encode, from_bytes, to_vec};
    use alloc::vec;
    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes T by value to keep call sites ergonomic; `assert_eq!` then borrows internally"
    )]
    fn rt<T: Encode + Decode + PartialEq + core::fmt::Debug>(v: T) {
        let bytes = to_vec(&v);
        let back: T = from_bytes(&bytes).expect("decode");
        assert_eq!(v, back);
    }
    #[test]
    fn primitive_widths() {
        assert_eq!(to_vec(&0u8), vec![0]);
        assert_eq!(to_vec(&1u8), vec![1]);
        assert_eq!(to_vec(&0x0102u16), vec![0x02, 0x01]);
        assert_eq!(to_vec(&0x0102_0304_u32), vec![0x04, 0x03, 0x02, 0x01]);
        assert_eq!(to_vec(&true), vec![1]);
        assert_eq!(to_vec(&false), vec![0]);
    }
    #[test]
    fn primitive_roundtrip() {
        rt(0u8);
        rt(255u8);
        rt(-1i8);
        rt(u16::MAX);
        rt(i16::MIN);
        rt(u32::MAX);
        rt(u64::MAX);
        rt(u128::MAX);
        rt(i128::MIN);
        rt(1.5f32);
        rt(f64::INFINITY.to_bits());
        rt(true);
        rt(false);
    }
    #[test]
    fn invalid_bool_rejected() {
        let err = from_bytes::<bool>(&[2u8]).unwrap_err();
        assert_eq!(err, DecodeError::TagOutOfRange { tag: 2 });
    }
}
