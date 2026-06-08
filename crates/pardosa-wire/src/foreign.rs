#[cfg(feature = "uuid")]
use crate::{Decode, DecodeError, Decoder, Encode, EventSafe, sealed};
#[cfg(feature = "uuid")]
use alloc::vec::Vec;
#[cfg(feature = "uuid")]
impl Encode for uuid::Uuid {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(uuid::Uuid::as_bytes(self));
    }
}
#[cfg(feature = "uuid")]
impl Decode for uuid::Uuid {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let bytes = d.read_bytes(16)?;
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Ok(uuid::Uuid::from_bytes(arr))
    }
}
#[cfg(feature = "uuid")]
impl sealed::Sealed for uuid::Uuid {}
#[cfg(feature = "uuid")]
impl EventSafe for uuid::Uuid {}
#[cfg(test)]
mod tests {
    #[cfg(feature = "uuid")]
    use crate::{DecodeError, from_bytes, to_vec};
    #[cfg(feature = "uuid")]
    #[test]
    fn uuid_roundtrip_and_layout() {
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
        let err = from_bytes::<uuid::Uuid>(&[0u8; 15]).unwrap_err();
        assert_eq!(err, DecodeError::BufferUnderflow);
    }
}
