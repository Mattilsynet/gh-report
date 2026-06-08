use crate::{Decode, DecodeError};
pub const DEFAULT_DECODE_CAP: usize = 1 << 20;
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    cap_remaining: usize,
}
impl<'a> Decoder<'a> {
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_cap(input, DEFAULT_DECODE_CAP)
    }
    #[must_use]
    pub fn with_cap(input: &'a [u8], cap: usize) -> Self {
        Self {
            input,
            pos: 0,
            cap_remaining: cap,
        }
    }
    /// Read `n` bytes from the underlying input.
    ///
    /// # Errors
    /// Returns `DecodeError::BufferUnderflow` when the cursor would advance past the end of input.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(DecodeError::BufferUnderflow)?;
        if end > self.input.len() {
            return Err(DecodeError::BufferUnderflow);
        }
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    /// Read a `u32` length prefix and check it against the remaining decode budget.
    ///
    /// # Errors
    /// Returns `DecodeError::BufferUnderflow` if the prefix itself cannot be read, or
    /// `DecodeError::LengthOutOfRange` if the decoded length exceeds the cap remaining.
    pub fn read_len_prefix(&mut self) -> Result<usize, DecodeError> {
        let raw = u32::decode(self)?;
        let n = raw as usize;
        if n > self.cap_remaining {
            return Err(DecodeError::LengthOutOfRange {
                len: raw,
                max: u32::try_from(self.cap_remaining).unwrap_or(u32::MAX),
            });
        }
        self.cap_remaining -= n;
        Ok(n)
    }
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }
    #[must_use]
    pub fn input_len(&self) -> usize {
        self.input.len()
    }
    #[must_use]
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.input.len()
    }
}
#[cfg(test)]
mod tests {
    use crate::{DecodeError, from_bytes, from_bytes_with_cap, to_vec};
    use alloc::vec;
    use alloc::vec::Vec;
    #[test]
    fn cap_exceeded_before_alloc() {
        let bogus_len: u32 = 2 * 1024 * 1024;
        let mut input = Vec::new();
        input.extend_from_slice(&bogus_len.to_le_bytes());
        let err = from_bytes::<Vec<u8>>(&input).unwrap_err();
        assert!(matches!(err, DecodeError::LengthOutOfRange { .. }));
    }
    #[test]
    fn cap_configurable() {
        let mut input = Vec::new();
        input.extend_from_slice(&8u32.to_le_bytes());
        input.extend_from_slice(&[0u8; 8]);
        let ok: Vec<u8> = from_bytes_with_cap(&input, 16).unwrap();
        assert_eq!(ok.len(), 8);
        let err = from_bytes_with_cap::<Vec<u8>>(&input, 4).unwrap_err();
        assert!(matches!(err, DecodeError::LengthOutOfRange { .. }));
    }
    #[test]
    fn cap_charges_nested_length_prefixes() {
        let v: Vec<Vec<u8>> = vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]];
        let bytes = to_vec(&v);
        let back: Vec<Vec<u8>> = from_bytes_with_cap(&bytes, 1024).unwrap();
        assert_eq!(back, v);
        let err = from_bytes_with_cap::<Vec<Vec<u8>>>(&bytes, 2).unwrap_err();
        assert!(matches!(err, DecodeError::LengthOutOfRange { .. }));
    }
}
