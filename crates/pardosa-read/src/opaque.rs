use pardosa_wire::{Decode, DecodeError, Decoder};
use serde::{Serialize, Serializer};
use std::fmt::Write as _;

/// Envelope-partial decode target for the `domain_event` field of a
/// [`pardosa::store::Event`] whose concrete payload type is unknown to
/// this crate (feynman orientation `adr-fmt-de91s`, approach C).
///
/// [`Decode`] consumes every remaining byte in the cursor rather than
/// interpreting them, so this type can stand in for any `T` when only
/// the envelope frame (`event_id`, `fiber_id`, `detached`, `precursor`,
/// `precursor_hash`) needs to be rendered. [`Serialize`] renders the
/// captured bytes as a lowercase hex string.
#[derive(Debug, Clone)]
pub struct OpaqueTail(Vec<u8>);

impl OpaqueTail {
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn hex(&self) -> String {
        let mut hex = String::with_capacity(self.0.len() * 2);
        for byte in &self.0 {
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}

impl Decode for OpaqueTail {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let remaining = d.input_len() - d.position();
        let bytes = d.read_bytes(remaining)?;
        Ok(OpaqueTail(bytes.to_vec()))
    }
}

impl Serialize for OpaqueTail {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}
