//! prod-loc — count Rust production lines, excluding tests.
//!
//! See README.md for the rules.

#![forbid(unsafe_code)]

pub mod parse;
pub mod walk;

use std::path::{Path, PathBuf};

/// Per-file LOC report.
#[derive(Debug, Clone)]
pub struct FileReport {
    pub path: PathBuf,
    pub total_lines: usize,
    pub production_lines: usize,
    pub test_lines: usize,
    pub excluded_reason: Option<&'static str>,
}

/// Aggregate over many files.
#[derive(Debug, Default)]
pub struct AggregateReport {
    pub files: Vec<FileReport>,
    pub total_production: usize,
    pub total_test: usize,
}

impl AggregateReport {
    #[must_use]
    pub fn total_files(&self) -> usize {
        self.files.len()
    }
}

/// Analyse a single `.rs` file. Returns a `FileReport`.
///
/// Caller is responsible for `tests/`-dir exclusion; this fn measures any
/// `.rs` source it is handed.
#[must_use]
pub fn analyse_file(path: &Path) -> FileReport {
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("WARN\tread error\t{}\t{}", path.display(), e);
            return FileReport {
                path: path.to_path_buf(),
                total_lines: 0,
                production_lines: 0,
                test_lines: 0,
                excluded_reason: Some("read error"),
            };
        }
    };

    let total_lines = count_lines(&source);

    let test_ranges = match parse::test_ranges_for_file(&source) {
        Ok(ranges) => ranges,
        Err(e) => {
            eprintln!("WARN\tparse error\t{}\t{e}", path.display());
            return FileReport {
                path: path.to_path_buf(),
                total_lines,
                production_lines: 0,
                test_lines: 0,
                excluded_reason: Some("parse error"),
            };
        }
    };

    let coalesced = coalesce_ranges(test_ranges);
    let test_lines: usize = coalesced
        .iter()
        .map(|(s, e)| e.saturating_sub(*s) + 1)
        .sum();
    let test_lines = test_lines.min(total_lines);
    let production_lines = total_lines.saturating_sub(test_lines);

    FileReport {
        path: path.to_path_buf(),
        total_lines,
        production_lines,
        test_lines,
        excluded_reason: None,
    }
}

/// Count physical lines. A file ending without a trailing newline still
/// counts its last line. An empty file is 0 lines.
#[must_use]
pub fn count_lines(source: &str) -> usize {
    if source.is_empty() {
        return 0;
    }
    let mut n = source.matches('\n').count();
    if !source.ends_with('\n') {
        n += 1;
    }
    n
}

/// Sort + merge overlapping / adjacent inclusive `(start, end)` line ranges.
#[must_use]
pub fn coalesce_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    let mut cur = ranges[0];
    for r in ranges.into_iter().skip(1) {
        if r.0 <= cur.1 + 1 {
            cur.1 = cur.1.max(r.1);
        } else {
            out.push(cur);
            cur = r;
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_lines_basic() {
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a"), 1);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("a\nb"), 2);
        assert_eq!(count_lines("a\nb\n"), 2);
        assert_eq!(count_lines("\n\n"), 2);
    }

    #[test]
    fn coalesce_overlapping() {
        assert_eq!(coalesce_ranges(vec![]), Vec::<(usize, usize)>::new());
        assert_eq!(coalesce_ranges(vec![(1, 3)]), vec![(1, 3)]);
        assert_eq!(coalesce_ranges(vec![(1, 5), (3, 4)]), vec![(1, 5)]);
        assert_eq!(coalesce_ranges(vec![(1, 3), (4, 6)]), vec![(1, 6)]); // adjacent
        assert_eq!(coalesce_ranges(vec![(1, 3), (6, 8)]), vec![(1, 3), (6, 8)]);
        assert_eq!(
            coalesce_ranges(vec![(10, 12), (1, 3), (2, 4)]),
            vec![(1, 4), (10, 12)]
        );
    }
}
