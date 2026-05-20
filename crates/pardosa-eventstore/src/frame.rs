//! Length-prefixed xxh64-framed binary records.
//!
//! Wire format per frame: `[u32 len_le][bytes][u64 xxh64_le]`.
//! `len` is the body length in bytes; `xxh64` is `xxhash_rust::xxh64::xxh64(body, 0)`.
//!
//! `read_all_frames_valid` is recovery-oriented: it returns every
//! well-formed frame in append order and the byte offset of the last
//! valid frame boundary. A truncated tail (short header, short body,
//! or hash mismatch) terminates the scan without raising an error —
//! the caller truncates the file to `valid_end_offset` to discard the
//! torn tail.

use std::io::{self, Read, Write};

use xxhash_rust::xxh64::xxh64;

/// Length-prefix + body + xxh64 trailer overhead (frame-level bytes
/// added beyond the body itself).
const FRAME_OVERHEAD: usize = 4 + 8;

/// Write one frame: `[u32 len_le][body][u64 xxh64_le]`.
///
/// # Errors
///
/// Propagates the underlying writer's I/O error from any of the three
/// `write_all` calls. The frame is **not** transactional at this layer —
/// a `write_all` failure mid-frame leaves the writer at an indeterminate
/// boundary. Callers requiring atomicity per frame must buffer into a
/// `Vec<u8>` first and issue one `write_all`.
///
/// # Panics
///
/// Panics if `body.len()` exceeds `u32::MAX` — the frame length prefix
/// is `u32` LE. Realistic envelope sizes are far below this bound.
pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> io::Result<()> {
    let len_u32 = u32::try_from(body.len()).expect("frame body length exceeds u32::MAX");
    let hash = xxh64(body, 0);
    w.write_all(&len_u32.to_le_bytes())?;
    w.write_all(body)?;
    w.write_all(&hash.to_le_bytes())?;
    Ok(())
}

/// Read every well-formed frame from `r`, in order.
///
/// Returns the decoded bodies and the byte offset of the last valid
/// frame boundary. Recovery contract:
///
/// - Empty reader → `(vec![], 0)`.
/// - Trailing torn header / short body / xxh64 mismatch → stop scanning,
///   return all bodies read so far, and report the offset that survives
///   truncation. A subsequent `set_len(valid_end_offset)` on the file
///   removes the torn tail.
/// - Genuine I/O errors (anything other than `UnexpectedEof` from
///   `read_exact`) surface as `Err`.
///
/// # Errors
///
/// Returns the underlying reader's I/O error for non-EOF read failures.
pub fn read_all_frames_valid<R: Read>(r: &mut R) -> io::Result<(Vec<Vec<u8>>, u64)> {
    let mut bodies = Vec::new();
    let mut valid_end: u64 = 0;
    loop {
        let mut len_buf = [0u8; 4];
        match read_full_or_eof(r, &mut len_buf)? {
            FrameRead::Eof | FrameRead::Short => return Ok((bodies, valid_end)),
            FrameRead::Full => {}
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        // Allocate the body buffer lazily — a malformed huge `len` would
        // attempt a giant allocation, but a torn-tail recovery is bounded
        // by the file size the caller passes us. We accept the risk:
        // the xxh64 mismatch on the trailer will catch garbage.
        let mut body = vec![0u8; len];
        match read_full_or_eof(r, &mut body)? {
            FrameRead::Eof | FrameRead::Short => return Ok((bodies, valid_end)),
            FrameRead::Full => {}
        }
        let mut hash_buf = [0u8; 8];
        match read_full_or_eof(r, &mut hash_buf)? {
            FrameRead::Eof | FrameRead::Short => return Ok((bodies, valid_end)),
            FrameRead::Full => {}
        }
        let stored = u64::from_le_bytes(hash_buf);
        let computed = xxh64(&body, 0);
        if stored != computed {
            // Trailer mismatch — torn or corrupt tail. Stop here so the
            // caller can truncate to `valid_end` and reopen for append.
            return Ok((bodies, valid_end));
        }
        valid_end += (FRAME_OVERHEAD + len) as u64;
        bodies.push(body);
    }
}

enum FrameRead {
    Full,
    Short,
    Eof,
}

/// Read exactly `buf.len()` bytes, distinguishing clean EOF (zero bytes
/// read at a frame boundary) from a torn tail (partial bytes mid-frame).
fn read_full_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<FrameRead> {
    let mut read = 0;
    while read < buf.len() {
        match r.read(&mut buf[read..]) {
            Ok(0) => {
                return Ok(if read == 0 {
                    FrameRead::Eof
                } else {
                    FrameRead::Short
                });
            }
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(FrameRead::Full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn empty_reader_yields_no_bodies() {
        let mut r = Cursor::new(Vec::<u8>::new());
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert!(bodies.is_empty());
        assert_eq!(offset, 0);
    }

    #[test]
    fn single_frame_roundtrip() {
        let body = b"hello, world";
        let mut buf = Vec::new();
        write_frame(&mut buf, body).unwrap();
        assert_eq!(buf.len(), 4 + body.len() + 8);

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![body.to_vec()]);
        assert_eq!(offset, buf.len() as u64);
    }

    #[test]
    fn two_frames_in_order() {
        let a = b"first".to_vec();
        let b = b"second body, longer".to_vec();
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        write_frame(&mut buf, &b).unwrap();

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![a, b]);
        assert_eq!(offset, buf.len() as u64);
    }

    #[test]
    fn empty_body_frame_roundtrips() {
        // Zero-length body is a valid frame: 4-byte len(0) + 0 body + 8-byte hash.
        let mut buf = Vec::new();
        write_frame(&mut buf, &[]).unwrap();
        assert_eq!(buf.len(), 12);

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![Vec::<u8>::new()]);
        assert_eq!(offset, 12);
    }

    #[test]
    fn torn_tail_truncated_in_body_recovers_prefix() {
        let a = b"keep".to_vec();
        let b = b"torn-body".to_vec();
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        let prefix_end = buf.len();
        write_frame(&mut buf, &b).unwrap();
        // Drop the last hash byte AND the last body byte → truncated mid-body.
        buf.truncate(prefix_end + 4 + b.len() - 1);

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![a]);
        assert_eq!(offset, prefix_end as u64);
    }

    #[test]
    fn torn_tail_truncated_in_header_recovers_prefix() {
        let a = b"keep".to_vec();
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        let prefix_end = buf.len();
        // Write a partial header (2 bytes of 4) — torn tail in the length prefix.
        buf.extend_from_slice(&[0xff, 0xff]);

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![a]);
        assert_eq!(offset, prefix_end as u64);
    }

    #[test]
    fn bad_xxh64_trailer_truncates_at_prior_boundary() {
        let a = b"keep".to_vec();
        let b = b"will-corrupt".to_vec();
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).unwrap();
        let prefix_end = buf.len();
        write_frame(&mut buf, &b).unwrap();
        // Corrupt the last byte of the trailing xxh64 → mismatch.
        let last = buf.len() - 1;
        buf[last] ^= 0xff;

        let mut r = Cursor::new(&buf);
        let (bodies, offset) = read_all_frames_valid(&mut r).unwrap();
        assert_eq!(bodies, vec![a]);
        assert_eq!(offset, prefix_end as u64);
    }
}

#[cfg(test)]
mod proptests {
    //! Property-level coverage of the frame codec round-trip.
    //!
    //! Two invariants:
    //! 1. **Round-trip.** Any sequence of bodies, written in order, reads
    //!    back exactly as written; `valid_end` equals the full buffer
    //!    length.
    //! 2. **Tail-truncation recovery.** Writing N frames then truncating
    //!    *anywhere* inside the last frame's bytes recovers the first
    //!    N-1 bodies exactly, with `valid_end` pointing at the boundary
    //!    between frame N-1 and frame N.

    use super::{FRAME_OVERHEAD, read_all_frames_valid, write_frame};
    use proptest::collection::vec as prop_vec;
    use proptest::prelude::*;
    use std::io::Cursor;

    proptest! {
        #[test]
        fn round_trip_arbitrary_bodies(bodies in prop_vec(prop_vec(any::<u8>(), 0..256), 0..16)) {
            let mut buf = Vec::new();
            for b in &bodies {
                write_frame(&mut buf, b).unwrap();
            }
            let total_len = buf.len() as u64;
            let mut r = Cursor::new(&buf);
            let (read_bodies, valid_end) = read_all_frames_valid(&mut r).unwrap();
            prop_assert_eq!(read_bodies, bodies);
            prop_assert_eq!(valid_end, total_len);
        }

        #[test]
        fn torn_tail_recovers_prefix(
            bodies in prop_vec(prop_vec(any::<u8>(), 0..64), 1..8),
            truncate_inside_last in 1u64..16,
        ) {
            // Build the full buffer and remember the boundary at the end
            // of frame N-1.
            let mut buf = Vec::new();
            let mut prefix_end = 0u64;
            for (i, b) in bodies.iter().enumerate() {
                if i + 1 == bodies.len() {
                    prefix_end = buf.len() as u64;
                }
                write_frame(&mut buf, b).unwrap();
            }
            // Truncate strictly inside the last frame's bytes.
            let last_frame_size = (buf.len() as u64) - prefix_end;
            let chop = (truncate_inside_last % last_frame_size.max(1)).max(1);
            let new_len = usize::try_from((buf.len() as u64) - chop).unwrap();
            buf.truncate(new_len);

            let mut r = Cursor::new(&buf);
            let (read_bodies, valid_end) = read_all_frames_valid(&mut r).unwrap();
            let expected: Vec<Vec<u8>> = bodies[..bodies.len() - 1].to_vec();
            prop_assert_eq!(read_bodies, expected);
            prop_assert_eq!(valid_end, prefix_end);
        }

        #[test]
        fn frame_overhead_is_12(body_len in 0usize..64) {
            // Documented invariant: header(4) + trailer(8) = 12.
            let body = vec![0u8; body_len];
            let mut buf = Vec::new();
            write_frame(&mut buf, &body).unwrap();
            prop_assert_eq!(buf.len(), body_len + FRAME_OVERHEAD);
        }
    }
}
