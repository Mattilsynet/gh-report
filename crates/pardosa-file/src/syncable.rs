//! Substrate-pure durability surface.
//!
//! [`Syncable`] separates the buffered-sink contract ([`Write::flush`])
//! from the host's sync primitive ([`File::sync_data`]), plus [`Syncable::set_len`]
//! so rewrite-from-zero callers drop stale trailing bytes before sync.
//!
//! [`fsync_parent_dir`] fences directory-entry creation by `sync_data`-ing
//! the parent directory handle.
//!
//! Per ADR-0010 §D3 (platform caveats), ADR-0002 (handcrafted trait), and
//! ADR-0014 §F3 (sealed via [`sealed::Sealed`]).
use std::fs::File;
use std::io::{self, BufWriter, Cursor, Write};
use std::path::Path;
/// `sync_data` the directory at `dir` so directory-entry mutations
/// under it (creates, renames, unlinks) are durable per the host
/// platform's POSIX-equivalent contract.
///
/// On Unix this opens `dir` read-only and calls [`File::sync_data`]
/// on the directory handle (per ADR-0010 §D3 platform caveats). On
/// Windows the call is a no-op — directory fsync has no FS-level
/// analogue.
///
/// # Errors
/// On Unix, forwards any [`io::Error`] from opening `dir` or from the
/// directory `sync_data` call. On Windows, returns `Ok(())`
/// unconditionally.
#[cfg(not(windows))]
pub fn fsync_parent_dir(dir: &Path) -> io::Result<()> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let f = File::open(dir)?;
    f.sync_data()
}
#[cfg(windows)]
/// # Errors
/// Always returns `Ok(())` on Windows (no FS-level directory-fsync
/// analogue). Signature mirrors the Unix variant for substrate parity.
pub fn fsync_parent_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}
pub(crate) mod sealed {
    /// Private supertrait sealing [`super::Syncable`]: downstream
    /// crates cannot name `crate::syncable::sealed::Sealed`, so they
    /// cannot satisfy the supertrait bound on `Syncable`.
    pub trait Sealed {}
}
/// Substrate-pure durability surface. Composes [`Write`] with an
/// explicit `sync_data` that names the step between "bytes left this
/// process" and the platform's sync primitive returning `Ok`. See
/// ADR-0010 §D3 for per-platform caveats.
///
/// Sealed per ADR-0014: closed impl set is [`Vec<u8>`],
/// [`Cursor<Vec<u8>>`], [`File`], [`BufWriter<W>`], `&mut W`.
///
/// # Errors
/// `sync_data` and `set_len` return [`io::Error`] from the underlying
/// sink. Memory-backed impls cannot fail on `sync_data`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement Syncable",
    label = "the trait `Syncable` is not implemented for `{Self}`",
    note = "Syncable is sealed in pardosa-file; the closed impl set is Vec<u8>, Cursor<Vec<u8>>, std::fs::File, BufWriter<W: Syncable>, and &mut W. To extend, send a PR adding impl Syncable (including set_len) and impl sealed::Sealed in crates/pardosa-file/src/syncable.rs."
)]
pub trait Syncable: Write + sealed::Sealed {
    /// Invoke the host platform's file-data sync primitive on this
    /// sink. On-disk sinks delegate to [`File::sync_data`]; memory
    /// sinks delegate to [`Write::flush`].
    ///
    /// `Ok(())` reports that the platform call returned `Ok` for the
    /// bytes written so far; the exact durability guarantee depends
    /// on host platform and storage stack (see ADR-0010 §D3).
    ///
    /// # Errors
    /// Forwards any [`io::Error`] from the underlying sync call.
    fn sync_data(&mut self) -> io::Result<()>;
    /// Truncate the sink to exactly `len` bytes. Complement of
    /// `sync_data` for rewrite-from-zero callers: call `set_len(new_len)`
    /// before `sync_data` so trailing bytes from a longer prior
    /// rewrite do not survive into fsynced state.
    ///
    /// Semantics mirror [`File::set_len`]. Memory sinks truncate in
    /// place; growth is unspecified (substrate shrinks).
    ///
    /// # Errors
    /// Forwards [`io::Error`] from the truncation call.
    fn set_len(&mut self, len: u64) -> io::Result<()>;
}
impl sealed::Sealed for Vec<u8> {}
impl Syncable for Vec<u8> {
    fn sync_data(&mut self) -> io::Result<()> {
        Write::flush(self)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let new = usize::try_from(len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Syncable::set_len: len exceeds usize::MAX",
            )
        })?;
        if new <= self.len() {
            self.truncate(new);
            Ok(())
        } else {
            self.resize(new, 0);
            Ok(())
        }
    }
}
impl sealed::Sealed for Cursor<Vec<u8>> {}
impl Syncable for Cursor<Vec<u8>> {
    fn sync_data(&mut self) -> io::Result<()> {
        Write::flush(self)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        let inner = self.get_mut();
        <Vec<u8> as Syncable>::set_len(inner, len)
    }
}
impl sealed::Sealed for File {}
impl Syncable for File {
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(self)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        File::set_len(self, len)
    }
}
impl<W: Syncable> sealed::Sealed for BufWriter<W> {}
impl<W: Syncable> Syncable for BufWriter<W> {
    fn sync_data(&mut self) -> io::Result<()> {
        self.flush()?;
        self.get_mut().sync_data()
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.flush()?;
        self.get_mut().set_len(len)
    }
}
impl<W: Syncable + ?Sized> sealed::Sealed for &mut W {}
impl<W: Syncable + ?Sized> Syncable for &mut W {
    fn sync_data(&mut self) -> io::Result<()> {
        (**self).sync_data()
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        (**self).set_len(len)
    }
}
