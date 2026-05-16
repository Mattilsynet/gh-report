//! Filesystem walking: collect `.rs` files under a path, excluding any
//! subtree that contains a `tests` path component.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Walk `root` and return all `.rs` files outside `tests/` directories.
///
/// If `root` is itself a single file, return `vec![root]` (caller decides
/// whether to honour the `tests/` rule for explicit-file invocations).
#[must_use]
pub fn collect_rs_files(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_tests_dir(e.path()))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!("WARN\twalk error\t{err}");
                continue;
            }
        };
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|x| x == "rs") {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out
}

/// True if any path component is exactly `tests` AND that component is a
/// directory. We check by component name; `WalkDir::filter_entry` is
/// invoked on directory entries during descent, so a `tests/` dir gets
/// pruned before its contents are visited.
fn is_tests_dir(path: &Path) -> bool {
    // Only prune if the LAST component is `tests` and it's a directory.
    // We can't filesystem-check `is_dir()` reliably for the root, so just
    // match on the final component name.
    path.file_name().is_some_and(|n| n == "tests") && path.is_dir()
}

/// Standalone helper: is this path's *any* component equal to `tests`?
/// Used by callers that want to decide exclusion on a single `.rs` file
/// (e.g. an explicit-file invocation pointing at `crates/foo/tests/bar.rs`).
#[must_use]
pub fn path_has_tests_component(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "tests")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_tests_component() {
        assert!(path_has_tests_component(&PathBuf::from(
            "crates/foo/tests/bar.rs"
        )));
        assert!(path_has_tests_component(&PathBuf::from("tests/x.rs")));
    }

    #[test]
    fn ignores_src_tests_dot_rs() {
        // src/tests.rs is a FILE named tests.rs — not a tests/ DIR. Our
        // helper only matches components named `tests`, which `tests.rs`
        // is not.
        assert!(!path_has_tests_component(&PathBuf::from(
            "crates/foo/src/tests.rs"
        )));
    }

    #[test]
    fn ignores_unrelated_paths() {
        assert!(!path_has_tests_component(&PathBuf::from(
            "crates/foo/src/lib.rs"
        )));
    }
}
