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
