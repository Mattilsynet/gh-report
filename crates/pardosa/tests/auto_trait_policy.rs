//! Sentinel test enforcing ADR-0014 §F5: every `pub struct` / `pub enum`
//! declared in this crate must appear in the `assert_auto_traits!` block
//! in `src/lib.rs`. Pure text-scan inventory check on stable Rust; the
//! companion macro verifies built-in `Send`/`Sync` derivation only.
//!
//! See also: mission rescue-pardosa-59y0.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
const POLICY_BEGIN: &str = "// AUTO-TRAIT-POLICY-BEGIN";
const POLICY_END: &str = "// AUTO-TRAIT-POLICY-END";
fn crate_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}
fn lib_rs() -> PathBuf {
    crate_src_dir().join("lib.rs")
}
/// Recursively walk `src/` and collect every `pub struct NAME` / `pub enum NAME`
/// declaration name. Ignores items behind `#[cfg(test)]`.
fn declared_public_types() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(&crate_src_dir(), &mut out);
    out
}
fn walk(dir: &Path, out: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let text = fs::read_to_string(&path).expect("read source");
            extract_pub_types(&text, out);
        }
    }
}
fn extract_pub_types(text: &str, out: &mut BTreeSet<String>) {
    let stripped = strip_cfg_test_blocks(text);
    for line in stripped.lines() {
        let line = line.trim_start();
        if let Some(rest) = line
            .strip_prefix("pub struct ")
            .or_else(|| line.strip_prefix("pub enum "))
        {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
}
/// Crude `#[cfg(test)] mod NAME { ... }` block stripper. Brace-balanced.
fn strip_cfg_test_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"#[cfg(test)]") {
            let mut j = i + "#[cfg(test)]".len();
            while j < bytes.len() && bytes[j] != b'{' {
                if bytes[j] == b';' {
                    break;
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'{' {
                let mut depth = 1usize;
                j += 1;
                while j < bytes.len() && depth > 0 {
                    match bytes[j] {
                        b'{' => depth += 1,
                        b'}' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
/// Parse the leading identifier (before `<` or whitespace) from each type
/// expression inside `SendSync { ... }` / `SendOnly { ... }` / `NotSend { ... }`
/// inside the AUTO-TRAIT-POLICY block in `lib.rs`.
fn policy_listed_types() -> BTreeSet<String> {
    let text = fs::read_to_string(lib_rs()).expect("read lib.rs");
    let begin = text.find(POLICY_BEGIN).unwrap_or_else(|| {
        panic!(
            "{} marker not found in {}",
            POLICY_BEGIN,
            lib_rs().display()
        )
    });
    let end = text[begin..]
        .find(POLICY_END)
        .unwrap_or_else(|| panic!("{POLICY_END} marker not found after {POLICY_BEGIN}"));
    let block = &text[begin..begin + end];
    let block_owned = strip_line_comments(block);
    let block = block_owned.as_str();
    let mut out = BTreeSet::new();
    for kind in ["SendSync", "SendOnly", "NotSend"] {
        let pat = kind.to_string();
        let mut from = 0;
        while let Some(rel) = block[from..].find(&pat) {
            let after = from + rel + pat.len();
            let rest = block[after..].trim_start();
            if !rest.starts_with('{') {
                from = after;
                continue;
            }
            let inner_start = after + (block[after..].find('{').expect("brace") + 1);
            let inner_end = inner_start
                + block[inner_start..]
                    .find('}')
                    .expect("policy block: missing closing brace");
            let inner = &block[inner_start..inner_end];
            for item in split_top_level_commas(inner) {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                if let Some(name) = type_head_ident(item) {
                    out.insert(name);
                }
            }
            from = inner_end;
        }
    }
    out
}
/// Extract the type's head identifier from a type expression, ignoring
/// generic arguments and module path prefix:
///
/// - `Foo`                → `Foo`
/// - `Foo<u64>`           → `Foo`
/// - `persist::Error`     → `Error`
/// - `mod::Foo<'a, T>`    → `Foo`
/// - `Reader<std::io::Cursor<Vec<u8>>>` → `Reader`
fn type_head_ident(item: &str) -> Option<String> {
    let head = match item.find('<') {
        Some(i) => &item[..i],
        None => item,
    }
    .trim();
    let last = head.rsplit("::").next()?.trim();
    let name: String = last
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        None
    } else {
        Some(name)
    }
}
/// Strip `// ... \n` line comments from a slice, replacing them with
/// equally-sized whitespace (preserves spans for the brace matcher).
fn strip_line_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}
/// Split a comma-separated list of type expressions at top-level commas,
/// respecting `<...>` angle-bracket nesting so commas inside generic args
/// (e.g. `EventVec<u64, 8>`) don't split the item.
fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, ch) in text.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}
#[test]
fn every_public_type_appears_in_auto_trait_policy() {
    let declared = declared_public_types();
    let listed = policy_listed_types();
    let missing: Vec<_> = declared.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&declared).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "auto-trait policy drift:\n  missing (declared but not in policy): {missing:?}\n  stale (in policy but not declared): {stale:?}"
    );
}
