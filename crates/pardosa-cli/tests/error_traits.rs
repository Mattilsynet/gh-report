//! Cross-crate error-trait invariants.
//!
//! Lives in the "depends on everything" crate so one test
//! target covers every error type.
//!
//! 1. Every workspace error impls `core::error::Error` (not
//!    `std::error::Error`). Substrate crates are `no_std`;
//!    `std::` would breach purity
//!    (`pardosa-error-rearch`). Compile-time
//!    `assert_core_error::<T>()`.
//! 2. No `GenomeSafe`-impling type embeds a workspace error
//!    as a field — errors are runtime values; embedding folds
//!    a non-canonical surface into the schema hash. Textual
//!    walk over `crates/**/src/**.rs`. Bead
//!    `rescue-pardosa-edn`.
use pardosa::store::PardosaError;
use pardosa_schema::DomainError;
fn assert_core_error<T: core::error::Error>() {}
#[allow(dead_code)]
fn _check_all_errors_impl_core_error() {
    assert_core_error::<DomainError>();
    assert_core_error::<PardosaError>();
}
/// Workspace error types whose embedding inside a `GenomeSafe`-impling type
/// would corrupt the schema hash surface. Mirrors the set checked in (1),
/// extended to the substrate error types whose visibility is now `pub(crate)`
/// in `pardosa` but which still exist and must not leak into the schema-hash
/// surface of any `GenomeSafe`-impling type anywhere in the workspace.
const ERROR_TYPE_NAMES: &[&str] = &[
    "DecodeError",
    "DomainError",
    "FileError",
    "PardosaError",
    "FiberInvariantKind",
];
/// Returns the workspace root by walking up from `CARGO_MANIFEST_DIR` until a
/// directory containing a `Cargo.toml` with `[workspace]` is found.
fn workspace_root() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut p = manifest.as_path();
    loop {
        let candidate = p.join("Cargo.toml");
        if candidate.exists() {
            let s = std::fs::read_to_string(&candidate).unwrap_or_default();
            if s.contains("[workspace]") {
                return p.to_path_buf();
            }
        }
        p = p
            .parent()
            .expect("walked past filesystem root looking for workspace");
    }
}
/// Walk a directory recursively, returning paths to all `.rs` files under
/// `src/` subtrees within `crates/`.
fn collect_src_rs_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let crates_root = root.join("crates");
    let mut out = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![crates_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let parent_in_crates =
                    path.parent().and_then(|p| p.parent()) == Some(&root.join("crates"));
                if name == "src" || !parent_in_crates {
                    stack.push(path);
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}
#[test]
fn no_genome_safe_type_embeds_an_error_type() {
    let root = workspace_root();
    let files = collect_src_rs_files(&root);
    assert!(!files.is_empty(), "found no .rs files under crates/*/src/");
    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        let Ok(src) = std::fs::read_to_string(file) else {
            continue;
        };
        let mut genome_safe_types: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for line in src.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("impl")
                && let Some(idx) = rest.find("GenomeSafe for ")
            {
                let after = &rest[idx + "GenomeSafe for ".len()..];
                let ty = after
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !ty.is_empty() {
                    genome_safe_types.insert(ty);
                }
            }
        }
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if t.starts_with("#[derive(") && t.contains("GenomeSafe") {
                for next in lines.iter().take((i + 12).min(lines.len())).skip(i + 1) {
                    let nt = next.trim_start();
                    let kw = if nt.starts_with("pub struct ") {
                        Some("pub struct ")
                    } else if nt.starts_with("struct ") {
                        Some("struct ")
                    } else if nt.starts_with("pub enum ") {
                        Some("pub enum ")
                    } else if nt.starts_with("enum ") {
                        Some("enum ")
                    } else {
                        None
                    };
                    if let Some(kw) = kw {
                        let rest = &nt[kw.len()..];
                        let ty = rest
                            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !ty.is_empty() {
                            genome_safe_types.insert(ty);
                        }
                        break;
                    }
                }
            }
        }
        if genome_safe_types.is_empty() {
            continue;
        }
        for ty in &genome_safe_types {
            let mut decl_pos: Option<usize> = None;
            for kw in ["struct ", "enum "] {
                let needle = format!("{kw}{ty}");
                let mut search_from = 0usize;
                while let Some(pos) = src[search_from..].find(&needle) {
                    let abs = search_from + pos;
                    let after_idx = abs + needle.len();
                    let after = src[after_idx..].chars().next().unwrap_or(' ');
                    if matches!(after, ' ' | '<' | '{' | '(' | '\n') {
                        decl_pos = Some(abs);
                        break;
                    }
                    search_from = abs + needle.len();
                }
                if decl_pos.is_some() {
                    break;
                }
            }
            let Some(decl_pos) = decl_pos else { continue };
            let after_decl = &src[decl_pos..];
            let Some(rel_body_start) = after_decl.find(['{', '(']) else {
                continue;
            };
            let body_start = decl_pos + rel_body_start;
            let open = src.as_bytes()[body_start];
            let close = if open == b'{' { b'}' } else { b')' };
            let bytes = src.as_bytes();
            let mut depth = 0i32;
            let mut body_end = body_start;
            for (i, b) in bytes.iter().enumerate().skip(body_start) {
                if *b == open {
                    depth += 1;
                } else if *b == close {
                    depth -= 1;
                    if depth == 0 {
                        body_end = i;
                        break;
                    }
                }
            }
            let body = &src[body_start..=body_end];
            for err in ERROR_TYPE_NAMES {
                if body.contains(err) {
                    violations.push(format!(
                        "{}: GenomeSafe type `{ty}` body references error type `{err}`",
                        file.strip_prefix(&root).unwrap_or(file).display()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "GenomeSafe-impling types must not embed workspace error types:\n  - {}",
        violations.join("\n  - ")
    );
}
