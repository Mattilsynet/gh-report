//! Negative pin for CHE-0023 (termination-is-domain-event).
//!
//! CHE-0023 forbids framework-style lifecycle termination in
//! `cherry-pit-core`: termination must be modelled as a domain event,
//! not a `fn is_terminated` predicate nor a `Terminated` lifecycle
//! variant carried in an ADT.
//!
//! This test scans `crates/cherry-pit-core/src/**/*.rs` for the
//! forbidden patterns and asserts zero occurrences. A future PR that
//! re-introduces either shape fails this test locally before review.
//!
//! Patterns mirror `rg '\bfn is_terminated\b|\bTerminated\s*[,}]'`.

use std::fs;
use std::path::{Path, PathBuf};

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn is_word_boundary(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(ch) => !(ch.is_alphanumeric() || ch == '_'),
    }
}

/// Matches `\bfn is_terminated\b`.
fn has_fn_is_terminated(line: &str) -> bool {
    let needle = "fn is_terminated";
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(pos) = line[i..].find(needle) {
        let start = i + pos;
        let end = start + needle.len();
        let before = if start == 0 {
            None
        } else {
            line[..start].chars().next_back()
        };
        let after = line[end..].chars().next();
        if is_word_boundary(before) && is_word_boundary(after) {
            return true;
        }
        i = start + 1;
        if i >= bytes.len() {
            break;
        }
    }
    false
}

/// Matches `\bTerminated\s*[,}]`.
fn has_terminated_variant(line: &str) -> bool {
    let needle = "Terminated";
    let mut i = 0;
    while let Some(pos) = line[i..].find(needle) {
        let start = i + pos;
        let end = start + needle.len();
        let before = if start == 0 {
            None
        } else {
            line[..start].chars().next_back()
        };
        if is_word_boundary(before) {
            let mut rest = line[end..].chars();
            let next_non_ws = rest.by_ref().find(|c| !c.is_whitespace());
            if matches!(next_non_ws, Some(',' | '}')) {
                return true;
            }
        }
        i = start + 1;
        if i >= line.len() {
            break;
        }
    }
    false
}

#[test]
fn termination_is_domain_event() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src, &mut files);
    assert!(!files.is_empty(), "no .rs files found under {src:?}");

    let mut hits: Vec<String> = Vec::new();
    for path in &files {
        let text = fs::read_to_string(path).expect("read source");
        for (n, line) in text.lines().enumerate() {
            if has_fn_is_terminated(line) || has_terminated_variant(line) {
                hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }

    assert!(
        hits.is_empty(),
        "CHE-0023 violation: framework-style termination found in cherry-pit-core/src:\n{}",
        hits.join("\n")
    );
}
