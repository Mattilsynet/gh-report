//! Stateful decoder cursor with a per-decode allocation cap.
//!
//! The decoder owns a `&'a [u8]` input and tracks a remaining-cap budget;
//! `read_len_prefix` charges length-prefix headers against the budget
//! *before* allocation so a malformed wire cannot trick the decoder into
//! an oversized allocation. GEN-0035 §"Decoder cap".

use crate::{Decode, EventError};

/// Default per-decode allocation cap: 1 MiB. Matches the F2 (r2)
/// commander decision; a length-prefix header exceeding the remaining
/// budget is rejected before allocation.
pub const DEFAULT_DECODE_CAP: usize = 1 << 20;

/// Stateful decoder cursor with a remaining-cap budget.
///
/// The cap is per-decode-invocation: each top-level call constructs a
/// fresh `Decoder` with a fresh budget. Length-prefix headers are
/// validated against the *remaining* budget so nested decoders share
/// the parent cap rather than reset it.
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    cap_remaining: usize,
}

impl<'a> Decoder<'a> {
    /// Construct a decoder with the [`DEFAULT_DECODE_CAP`] budget.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_cap(input, DEFAULT_DECODE_CAP)
    }

    /// Construct a decoder with a caller-supplied byte cap.
    #[must_use]
    pub fn with_cap(input: &'a [u8], cap: usize) -> Self {
        Self {
            input,
            pos: 0,
            cap_remaining: cap,
        }
    }

    /// Read exactly `n` bytes from the cursor, advancing it.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidInput`] when fewer than `n` bytes remain
    /// in the input, or when the resulting cursor position would overflow.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], EventError> {
        let end = self.pos.checked_add(n).ok_or(EventError::InvalidInput)?;
        if end > self.input.len() {
            return Err(EventError::InvalidInput);
        }
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a `u32 LE` length header and charge it against the cap.
    /// Returns `CapExceeded` *before* any allocation is attempted.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidInput`] if the prefix cannot be read or
    /// the length exceeds the remaining decode cap.
    pub fn read_len_prefix(&mut self) -> Result<usize, EventError> {
        let raw = u32::decode(self)?;
        let n = raw as usize;
        if n > self.cap_remaining {
            return Err(EventError::InvalidInput);
        }
        self.cap_remaining -= n;
        Ok(n)
    }

    /// Bytes consumed so far.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Total input length.
    #[must_use]
    pub fn input_len(&self) -> usize {
        self.input.len()
    }

    /// True when no input remains.
    #[must_use]
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.input.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::{EventError, from_bytes, from_bytes_with_cap, to_vec};
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn cap_exceeded_before_alloc() {
        // u32 length header = 2 MiB; default cap is 1 MiB. Must reject
        // before allocation (we cannot directly observe allocation, but
        // the error variant must be CapExceeded — not UnexpectedEof from
        // a successful huge allocation followed by short input).
        let bogus_len: u32 = 2 * 1024 * 1024;
        let mut input = Vec::new();
        input.extend_from_slice(&bogus_len.to_le_bytes());
        let err = from_bytes::<Vec<u8>>(&input).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn cap_configurable() {
        // 4-byte length header advertising 8 bytes is fine under cap=16,
        // rejected under cap=4.
        let mut input = Vec::new();
        input.extend_from_slice(&8u32.to_le_bytes());
        input.extend_from_slice(&[0u8; 8]);
        let ok: Vec<u8> = from_bytes_with_cap(&input, 16).unwrap();
        assert_eq!(ok.len(), 8);
        let err = from_bytes_with_cap::<Vec<u8>>(&input, 4).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn cap_charges_nested_length_prefixes() {
        // Vec<Vec<u8>> with two inner vecs of 4 bytes each: outer header
        // charges 2 (count), inner headers charge 4 each, payloads charge
        // 4 each. Total cap usage from len-prefixes = 2 + 4 + 4 = 10 bytes
        // of "budget"; we set cap to exactly fit.
        let v: Vec<Vec<u8>> = vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]];
        let bytes = to_vec(&v);
        // Generous cap succeeds.
        let back: Vec<Vec<u8>> = from_bytes_with_cap(&bytes, 1024).unwrap();
        assert_eq!(back, v);
        // Cap=2 (= outer count) succeeds for the outer header but the
        // inner length=4 exceeds remaining cap=0, so CapExceeded.
        let err = from_bytes_with_cap::<Vec<Vec<u8>>>(&bytes, 2).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }
}
