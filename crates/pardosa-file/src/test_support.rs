//! Test-only fault-injection [`Syncable`] sink.
//!
//! Compiled only with `--cfg test` or the `test-support` Cargo
//! feature. The surface is `#[doc(hidden)]` and explicitly outside
//! the semver guarantees of the public API (ADR-0009 §judgement-
//! primary). Used by `pardosa`'s journal rewrite-fence failure-
//! injection suite (rescue-pardosa-qf9h.8).
//!
//! See [`FailureSink`] for the configurable surface. Each operation
//! the journal rewrite path performs (`seek`, `write`, `stream_position`,
//! `set_len`, `sync_data`) can be made to fail on the *n*-th call so
//! tests can pin precise behaviour at each fence boundary.
use crate::Syncable;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
/// Counter that fires an injected failure after `n` non-failing
/// invocations. `Latch::Off` never fires; `Latch::FailAfter(n)`
/// allows `n` successful calls, then the (n+1)-th returns an error.
///
/// Counts are interior-mutable via `Cell` so tests can keep `&self`
/// observability across the trait impl boundary without `&mut self`
/// re-entry.
#[derive(Debug, Default)]
pub enum Latch {
    /// Never inject; pass through to the inner sink.
    #[default]
    Off,
    /// Permit `n` successful calls, then fail on the (n+1)-th.
    /// Subsequent calls continue to fail.
    FailAfter(std::cell::Cell<u32>),
}
impl Latch {
    /// Construct a latch that fails on the very next invocation.
    #[must_use]
    pub fn immediate() -> Self {
        Self::FailAfter(std::cell::Cell::new(0))
    }
    /// Construct a latch that fails on the (n+1)-th invocation.
    #[must_use]
    pub fn after(n: u32) -> Self {
        Self::FailAfter(std::cell::Cell::new(n))
    }
    fn should_fail(&self) -> bool {
        match self {
            Self::Off => false,
            Self::FailAfter(remaining) => {
                let r = remaining.get();
                if r == 0 {
                    true
                } else {
                    remaining.set(r - 1);
                    false
                }
            }
        }
    }
}
/// Configurable per-method failure policy for a [`FailureSink`].
#[derive(Debug, Default)]
pub struct FailurePlan {
    /// Trip [`Read::read`]. Rare in the rewrite path but supported
    /// for completeness.
    pub on_read: Latch,
    /// Trip [`Write::write`]. Fires during the `persist::persist`
    /// pass that streams body bytes through the sink.
    pub on_write: Latch,
    /// Trip [`Write::flush`]. Fires between `persist::persist` and
    /// the durability fence.
    pub on_flush: Latch,
    /// Trip [`Seek::seek`]. Fires on the journal's `SeekFrom::Start(0)`
    /// rewind before each rewrite.
    pub on_seek: Latch,
    /// Trip [`Seek::stream_position`]. Fires after the persist pass
    /// when the journal samples the new logical end-of-file.
    pub on_stream_position: Latch,
    /// Trip [`Syncable::set_len`]. Fires after `stream_position` and
    /// before `sync_data` — the truncation fence (W2).
    pub on_set_len: Latch,
    /// Trip [`Syncable::sync_data`]. Fires at the durability fence
    /// itself; corresponds to a failed `fdatasync` on a real disk.
    pub on_sync_data: Latch,
}
/// In-memory [`Syncable`] + [`Seek`] sink whose operations can each
/// be made to fail on a chosen invocation. The inner storage is a
/// [`Cursor<Vec<u8>>`] so bytes successfully written before the
/// failure are still observable via [`Self::contents`].
///
/// All injected failures surface as [`io::Error`] with
/// [`io::ErrorKind::Other`] and a fixed message tag per operation so
/// tests can match the cause without re-deriving the path through
/// the journal.
///
/// Sealed via the same private mechanism that seals every other
/// `Syncable` impl in this crate.
#[derive(Debug, Default)]
pub struct FailureSink {
    inner: Cursor<Vec<u8>>,
    plan: FailurePlan,
}
impl FailureSink {
    /// Construct an empty sink with no failure latches armed; every
    /// operation passes through.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Construct an empty sink with the supplied failure plan armed.
    #[must_use]
    pub fn with_plan(plan: FailurePlan) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            plan,
        }
    }
    /// Borrow the underlying bytes that have been successfully
    /// written so far.
    #[must_use]
    pub fn contents(&self) -> &[u8] {
        self.inner.get_ref().as_slice()
    }
    /// Borrow the active failure plan; useful for asserting the
    /// remaining count of an armed latch did or did not decrement.
    #[must_use]
    pub fn plan(&self) -> &FailurePlan {
        &self.plan
    }
}
fn fail(tag: &'static str) -> io::Error {
    io::Error::other(tag)
}
impl Read for FailureSink {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.plan.on_read.should_fail() {
            return Err(fail("FailureSink::read injected failure"));
        }
        self.inner.read(buf)
    }
}
impl Write for FailureSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.plan.on_write.should_fail() {
            return Err(fail("FailureSink::write injected failure"));
        }
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.plan.on_flush.should_fail() {
            return Err(fail("FailureSink::flush injected failure"));
        }
        self.inner.flush()
    }
}
impl Seek for FailureSink {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.plan.on_seek.should_fail() {
            return Err(fail("FailureSink::seek injected failure"));
        }
        self.inner.seek(pos)
    }
    fn stream_position(&mut self) -> io::Result<u64> {
        if self.plan.on_stream_position.should_fail() {
            return Err(fail("FailureSink::stream_position injected failure"));
        }
        self.inner.stream_position()
    }
}
impl crate::syncable::sealed::Sealed for FailureSink {}
impl Syncable for FailureSink {
    fn sync_data(&mut self) -> io::Result<()> {
        if self.plan.on_sync_data.should_fail() {
            return Err(fail("FailureSink::sync_data injected failure"));
        }
        Write::flush(&mut self.inner)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        if self.plan.on_set_len.should_fail() {
            return Err(fail("FailureSink::set_len injected failure"));
        }
        <Cursor<Vec<u8>> as Syncable>::set_len(&mut self.inner, len)
    }
}
