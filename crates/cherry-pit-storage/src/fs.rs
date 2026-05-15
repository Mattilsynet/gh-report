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

    trace!(path = %path.display(), "atomic write complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_write_bytes_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.bin");
        atomic_write_bytes(&path, b"hello").unwrap();
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"hello");
    }

    #[test]
    fn atomic_write_text_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        atomic_write_text(&path, "hello text").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello text");
    }

    #[test]
    fn atomic_write_creates_nested_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("file.txt");
        atomic_write_bytes(&path, b"nested").unwrap();
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"nested");
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("overwrite.bin");
        atomic_write_bytes(&path, b"first").unwrap();
        atomic_write_bytes(&path, b"second").unwrap();
        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"second");
    }

    #[test]
    fn atomic_write_empty_data() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        atomic_write_bytes(&path, b"").unwrap();
        let content = std::fs::read(&path).unwrap();
        assert!(content.is_empty());
    }
}
