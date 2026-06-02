//! Filesystem utilities: atomic writes, directory management.

use std::fs;
use std::io::Write;
use std::path::Path;

use tracing::trace;

use crate::error::PersistenceError;

/// Atomically write text content to a file using temp-file + rename.
///
/// Convenience wrapper over [`atomic_write_bytes`] for string content.
/// Primarily used by test fixtures.
///
/// # Errors
///
/// Returns `PersistenceError` if directory creation, temp file creation,
/// writing, flushing, or persisting the file fails.
pub fn atomic_write_text(path: &Path, content: &str) -> Result<(), PersistenceError> {
    trace!(path = %path.display(), bytes = content.len(), "atomic write (text)");
    atomic_write_bytes(path, content.as_bytes())
}

/// Atomically write raw bytes to a file using temp-file + rename.
///
/// # Errors
///
/// Returns `PersistenceError` if directory creation, temp file creation,
/// writing, flushing, or persisting the file fails.
pub fn atomic_write_bytes(path: &Path, data: &[u8]) -> Result<(), PersistenceError> {
    trace!(path = %path.display(), bytes = data.len(), "atomic write (bytes)");
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(PersistenceError::Io)?;

    let mut temp =
        tempfile::NamedTempFile::new_in(dir).map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to create temp file: {e}"),
        })?;

    temp.write_all(data)
        .map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to write temp file: {e}"),
        })?;

    temp.flush()
        .map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to flush temp file: {e}"),
        })?;

    temp.as_file()
        .sync_all()
        .map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to fsync temp file: {e}"),
        })?;

    temp.persist(path)
        .map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to persist temp file: {e}"),
        })?;

    fsync_parent_dir(dir)?;

    trace!(path = %path.display(), "atomic write complete");
    Ok(())
}

fn fsync_parent_dir(dir: &Path) -> Result<(), PersistenceError> {
    let dir_handle = fs::File::open(dir).map_err(|e| PersistenceError::AtomicWriteFailed {
        reason: format!("failed to open parent dir for fsync: {e}"),
    })?;
    dir_handle
        .sync_all()
        .map_err(|e| PersistenceError::AtomicWriteFailed {
            reason: format!("failed to fsync parent dir: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_creates_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("file.txt");
        atomic_write_bytes(&path, b"nested").unwrap();
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"nested");
    }

    #[test]
    fn atomic_write_recreates_removed_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("file.bin");
        atomic_write_bytes(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        let parent = path.parent().unwrap().to_path_buf();
        std::fs::remove_dir_all(&parent).unwrap();
        atomic_write_bytes(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }
}
