//! Subprocess + file-IO helpers used by criterion runners.

use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

/// Run a subprocess with workdir set, capturing stdout+stderr.
/// Panics with an informative message if spawn fails — calling agent
/// will see the panic and understand the env precondition.
pub fn run(workdir: &Path, program: &str, args: &[&str]) -> (Output, u128) {
    let start = Instant::now();
    let out = Command::new(program)
        .args(args)
        .current_dir(workdir)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn `{program}`: {e}. Ensure it is on PATH; \
                 track4-verify assumes `cargo`, `rg`, and `git` are available."
            )
        });
    (out, start.elapsed().as_millis())
}

/// Count newlines in a file. Panics if the file is missing — that's a
/// repo-shape precondition error worth being loud about.
#[must_use]
pub fn count_lines(path: &Path) -> usize {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "failed to read {}: {e}. track4-verify assumes Track 4 workspace shape.",
            path.display()
        )
    });
    if content.is_empty() {
        return 0;
    }
    // Match `wc -l` semantics: count newline chars.
    content.bytes().filter(|b| *b == b'\n').count()
}

/// Count rg-style matches in stdout. rg's default output is one match per
/// line: `path:line:content` (with `-n`). We count non-empty lines.
#[must_use]
pub fn count_rg_matches(stdout: &[u8]) -> usize {
    let s = std::str::from_utf8(stdout).unwrap_or("");
    s.lines().filter(|l| !l.is_empty()).count()
}

/// Search a UTF-8 text file for a literal substring. Returns Some(line_number)
/// for the first hit, or None. Returns Err if the file cannot be read.
pub fn find_substring(path: &Path, needle: &str) -> Result<Option<usize>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    for (i, line) in content.lines().enumerate() {
        if line.contains(needle) {
            return Ok(Some(i + 1));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_lines_synthetic() {
        let dir = tempdir();
        let p = dir.join("f.txt");
        std::fs::write(&p, "a\nb\nc\n").unwrap();
        assert_eq!(count_lines(&p), 3);
    }

    #[test]
    fn count_lines_no_trailing_newline() {
        let dir = tempdir();
        let p = dir.join("f.txt");
        std::fs::write(&p, "a\nb").unwrap();
        assert_eq!(count_lines(&p), 1);
    }

    #[test]
    fn count_lines_empty() {
        let dir = tempdir();
        let p = dir.join("f.txt");
        std::fs::write(&p, "").unwrap();
        assert_eq!(count_lines(&p), 0);
    }

    #[test]
    fn rg_count_matches_typical_output() {
        let stdout = b"path/a.rs:12:hit one\npath/b.rs:30:hit two\npath/b.rs:31:hit three\n";
        assert_eq!(count_rg_matches(stdout), 3);
    }

    #[test]
    fn rg_count_matches_empty() {
        assert_eq!(count_rg_matches(b""), 0);
    }

    #[test]
    fn find_substring_hit() {
        let dir = tempdir();
        let p = dir.join("f.md");
        std::fs::write(&p, "line one\nTrack 4 lives here\nline three\n").unwrap();
        assert_eq!(find_substring(&p, "Track 4").unwrap(), Some(2));
    }

    #[test]
    fn find_substring_miss() {
        let dir = tempdir();
        let p = dir.join("f.md");
        std::fs::write(&p, "nothing relevant\n").unwrap();
        assert_eq!(find_substring(&p, "Track 4").unwrap(), None);
    }

    /// Tiny tempdir shim so we don't pull in `tempfile` as a dep.
    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("track4-verify-test-{nanos}-{:p}", &nanos));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
