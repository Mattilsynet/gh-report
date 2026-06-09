//! Path containment for config-supplied directory strings.
//!
//! `adr-fmt` joins relative paths from `adr-fmt.toml` (corpus root,
//! domain directories, stale directory) to a parent directory before
//! reading files. A malicious or buggy config with an absolute path,
//! a parent traversal (`..`), or a symlink escape could induce the
//! tool to read arbitrary files outside the intended boundary.
//!
//! This module provides [`contained_join`] which performs:
//!
//! 1. **Lexical checks** on the segment string: reject absolute
//!    paths and any segment containing `..` or current-dir (`.`)
//!    components beyond the start.
//! 2. **Canonical containment**: after joining, canonicalize both
//!    the ADR root and the joined target. Reject if the canonical
//!    target is not a descendant of the canonical root. This
//!    catches symlinks that escape the corpus.
//!
//! Errors are surfaced as [`ContainmentError`] with the offending
//! segment and a reason suitable for inclusion in user-facing
//! error messages.

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Reason a path was rejected by [`contained_join`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainmentError {
    /// Segment is an absolute path.
    Absolute(String),
    /// Segment contains a `..` component (parent traversal).
    ParentTraversal(String),
    /// Segment is empty.
    Empty,
    /// Canonicalization of the joined path failed.
    CanonicalizeFailed { segment: String, reason: String },
    /// Canonical target escapes the canonical root via symlink or
    /// otherwise resolves outside the ADR corpus.
    EscapesRoot {
        segment: String,
        canonical_target: PathBuf,
        canonical_root: PathBuf,
    },
}

impl fmt::Display for ContainmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(s) => write!(
                f,
                "path {} is absolute; config directories must be relative to the ADR root",
                s.escape_debug()
            ),
            Self::ParentTraversal(s) => write!(
                f,
                "path {} contains a parent-traversal component (..); config directories must stay within the ADR root",
                s.escape_debug()
            ),
            Self::Empty => write!(f, "path segment is empty"),
            Self::CanonicalizeFailed { segment, reason } => {
                write!(
                    f,
                    "cannot canonicalize {}: {reason}",
                    segment.escape_debug()
                )
            }
            Self::EscapesRoot {
                segment,
                canonical_target,
                canonical_root,
            } => write!(
                f,
                "path {} resolves to {} which escapes the ADR root {} (likely via symlink)",
                segment.escape_debug(),
                canonical_target.display(),
                canonical_root.display()
            ),
        }
    }
}

impl std::error::Error for ContainmentError {}

/// Join `segment` to `root` after enforcing strict containment.
///
/// Returns the canonicalized target path on success. The target
/// must exist on disk (`std::fs::canonicalize` requires existence
/// on every supported platform); for paths that may legitimately
/// be absent at runtime, use [`contained_join_optional`] instead
/// of pre-checking with `Path::exists` (which would race the
/// canonicalize call).
pub fn contained_join(root: &Path, segment: &str) -> Result<PathBuf, ContainmentError> {
    lexical_check(segment)?;

    let joined = root.join(segment);
    let canonical_target =
        std::fs::canonicalize(&joined).map_err(|e| ContainmentError::CanonicalizeFailed {
            segment: segment.to_owned(),
            reason: e.to_string(),
        })?;
    let canonical_root =
        std::fs::canonicalize(root).map_err(|e| ContainmentError::CanonicalizeFailed {
            segment: segment.to_owned(),
            reason: format!("ADR root {}: {e}", root.display()),
        })?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err(ContainmentError::EscapesRoot {
            segment: segment.to_owned(),
            canonical_target,
            canonical_root,
        });
    }

    Ok(canonical_target)
}

/// Join `segment` to `root` after lexical checks; canonicalize
/// only if the target exists. Returns `Ok(None)` when the target
/// passes lexical checks but does not exist.
///
/// Used for paths that are optional at runtime (e.g., the stale
/// directory may not exist in a fresh repo).
pub fn contained_join_optional(
    root: &Path,
    segment: &str,
) -> Result<Option<PathBuf>, ContainmentError> {
    lexical_check(segment)?;

    let joined = root.join(segment);
    if !joined.exists() {
        return Ok(None);
    }

    contained_join(root, segment).map(Some)
}

/// Lexical-only validation: rejects absolute paths and `..`
/// components without touching the filesystem. Exposed separately
/// for callers that want to validate config strings before any
/// filesystem operation.
fn lexical_check(segment: &str) -> Result<(), ContainmentError> {
    if segment.is_empty() {
        return Err(ContainmentError::Empty);
    }

    let path = Path::new(segment);

    if path.is_absolute() {
        return Err(ContainmentError::Absolute(segment.to_owned()));
    }

    for component in path.components() {
        match component {
            Component::ParentDir => {
                return Err(ContainmentError::ParentTraversal(segment.to_owned()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ContainmentError::Absolute(segment.to_owned()));
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }

    #[test]
    fn rejects_absolute_path() {
        let dir = tmp();
        let err = contained_join(dir.path(), "/etc").unwrap_err();
        assert!(matches!(err, ContainmentError::Absolute(_)), "got: {err:?}");
    }

    #[test]
    fn rejects_parent_traversal() {
        let dir = tmp();
        let err = contained_join(dir.path(), "../etc").unwrap_err();
        assert!(
            matches!(err, ContainmentError::ParentTraversal(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn rejects_parent_traversal_mid_path() {
        let dir = tmp();
        let err = contained_join(dir.path(), "domain/../../etc").unwrap_err();
        assert!(
            matches!(err, ContainmentError::ParentTraversal(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn rejects_empty_segment() {
        let dir = tmp();
        let err = contained_join(dir.path(), "").unwrap_err();
        assert!(matches!(err, ContainmentError::Empty), "got: {err:?}");
    }

    #[test]
    fn accepts_normal_subdirectory() {
        let dir = tmp();
        let sub = dir.path().join("cherry");
        fs::create_dir(&sub).unwrap();
        let result = contained_join(dir.path(), "cherry").unwrap();
        assert!(result.starts_with(fs::canonicalize(dir.path()).unwrap()));
        assert!(result.ends_with("cherry"));
    }

    #[test]
    fn accepts_nested_subdirectory() {
        let dir = tmp();
        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        let result = contained_join(dir.path(), "a/b/c").unwrap();
        assert!(result.ends_with("a/b/c"));
    }

    #[test]
    fn rejects_canonicalize_missing_target() {
        let dir = tmp();
        let err = contained_join(dir.path(), "does-not-exist").unwrap_err();
        assert!(
            matches!(err, ContainmentError::CanonicalizeFailed { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn optional_join_returns_none_for_missing() {
        let dir = tmp();
        let result = contained_join_optional(dir.path(), "missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn optional_join_still_rejects_absolute() {
        let dir = tmp();
        let err = contained_join_optional(dir.path(), "/etc").unwrap_err();
        assert!(matches!(err, ContainmentError::Absolute(_)), "got: {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let parent = tmp();
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let root = parent.path().join("root");
        fs::create_dir(&root).unwrap();
        symlink(&outside, root.join("escape")).unwrap();

        let err = contained_join(&root, "escape").unwrap_err();
        assert!(
            matches!(err, ContainmentError::EscapesRoot { .. }),
            "got: {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepts_symlink_inside_root() {
        use std::os::unix::fs::symlink;

        let root = tmp();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, root.path().join("link")).unwrap();

        let result = contained_join(root.path(), "link").unwrap();
        assert!(result.starts_with(fs::canonicalize(root.path()).unwrap()));
    }

    #[test]
    fn cur_dir_component_allowed() {
        let dir = tmp();
        fs::create_dir(dir.path().join("domain")).unwrap();
        let result = contained_join(dir.path(), "./domain").unwrap();
        assert!(result.ends_with("domain"));
    }
}
