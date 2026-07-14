//! CI hard-gate: flag library `pub enum` error types that derive `thiserror::Error`
//! but lack `#[non_exhaustive]`, excluding `repr` and serde DTOs.
//!
//! Run `--help` for usage and the exact FLAG-IFF heuristic (see [`HELP_TEXT`]).
//! Only literal `pub enum` tokens `syn` can parse are inspected; macro-generated
//! enums are invisible to this tool, so under-flagging is the deliberate safe
//! failure mode for this hard CI gate. Governed by RST-0006 / PGN-0006 / CHE-0021.

use std::path::{Path, PathBuf};

const LIBRARY_CRATES: &[&str] = &[
    "crates/cherry-pit-core",
    "crates/cherry-pit-app",
    "crates/cherry-pit-gateway",
    "crates/cherry-pit-merger",
    "crates/cherry-pit-projection",
    "crates/cherry-pit-storage",
    "crates/cherry-pit-web",
    "crates/cherry-pit-wq",
    "crates/pardosa",
    "crates/pardosa-derive",
    "crates/pardosa-file",
    "crates/pardosa-wire",
    "crates/pardosa-schema",
    "crates/pardosa-nats",
    "crates/pardosa-fiber-store",
];

const HELP_TEXT: &str =
    "non-exhaustive-check - CI hard-gate for missing #[non_exhaustive] on error enums

USAGE:
    non-exhaustive-check [ROOT]

ROOT defaults to the workspace root, discovered by walking up from the
current directory until a Cargo.toml containing [workspace] is found.

HEURISTIC (FLAG-IFF):
    An enum is a violation iff:
        is_error_type && !has_non_exhaustive && !has_repr && !is_serde_dto

    has_non_exhaustive = any attribute path is exactly `non_exhaustive`
    has_repr           = any attribute path is exactly `repr`
    is_error_type      = any #[derive(..)] entry's last path segment is `Error`
    is_serde_dto       = any #[derive(..)] entry's last path segment is
                         `Serialize` or `Deserialize`

MACRO LIMITATION:
    macro-generated enums are not inspected; syn parses source tokens only.

OUTPUT:
    Exit 0 and a terse OK summary on stdout when clean.
    Exit 1 with one VIOLATION line per finding, tab-separated, then a summary.
";

#[derive(Debug)]
struct Violation {
    path: PathBuf,
    line: Option<usize>,
    enum_name: String,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP_TEXT}");
        return;
    }

    let root = match args.first() {
        Some(r) => PathBuf::from(r),
        None => default_root(),
    };

    let mut crates_scanned = 0usize;
    let mut enums_scanned = 0usize;
    let mut violations: Vec<Violation> = Vec::new();

    for crate_rel in LIBRARY_CRATES {
        let src_dir = root.join(crate_rel).join("src");
        if !src_dir.is_dir() {
            continue;
        }
        crates_scanned += 1;

        let mut rs_files: Vec<PathBuf> = Vec::new();
        collect_rs_files(&src_dir, &mut rs_files);
        rs_files.sort();

        for file in rs_files {
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
            let Ok(parsed) = syn::parse_file(&content) else {
                continue;
            };
            let mut enums: Vec<&syn::ItemEnum> = Vec::new();
            collect_enums(&parsed.items, &mut enums);
            for item_enum in enums {
                enums_scanned += 1;
                if is_violation(item_enum) {
                    let line = enum_span_line(item_enum);
                    let rel = file
                        .strip_prefix(&root)
                        .unwrap_or(file.as_path())
                        .to_path_buf();
                    violations.push(Violation {
                        path: rel,
                        line,
                        enum_name: item_enum.ident.to_string(),
                    });
                }
            }
        }
    }

    if violations.is_empty() {
        println!(
            "OK: {crates_scanned} library crates scanned, {enums_scanned} pub enums, 0 violations"
        );
        std::process::exit(0);
    }

    violations.sort_by_key(|v| (v.path.clone(), v.enum_name.clone()));
    for v in &violations {
        let loc = match v.line {
            Some(l) => format!("{}:{}", v.path.display(), l),
            None => v.path.display().to_string(),
        };
        println!(
            "VIOLATION\t{}\t{}\tmissing #[non_exhaustive]",
            loc, v.enum_name
        );
    }
    println!(
        "SUMMARY: {crates_scanned} library crates scanned, {enums_scanned} pub enums, {} violations",
        violations.len()
    );
    std::process::exit(1);
}

fn default_root() -> PathBuf {
    let start = std::env::current_dir().expect("current directory must be readable");
    let mut candidate = start.as_path();
    loop {
        let manifest = candidate.join("Cargo.toml");
        if manifest.is_file() {
            let content = std::fs::read_to_string(&manifest)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest.display()));
            if content.contains("[workspace]") {
                return candidate.to_path_buf();
            }
        }
        candidate = match candidate.parent() {
            Some(p) => p,
            None => panic!(
                "no workspace root (Cargo.toml with [workspace]) found above {}",
                start.display()
            ),
        };
    }
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry must read");
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn collect_enums<'a>(items: &'a [syn::Item], out: &mut Vec<&'a syn::ItemEnum>) {
    for item in items {
        match item {
            syn::Item::Enum(e) => out.push(e),
            syn::Item::Mod(m) => {
                if m.attrs.iter().any(is_cfg_test) {
                    continue;
                }
                if let Some((_, inner_items)) = &m.content {
                    collect_enums(inner_items, out);
                }
            }
            _ => {}
        }
    }
}

fn attr_path_is(attr: &syn::Attribute, name: &str) -> bool {
    attr.path().is_ident(name)
}

fn is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr_path_is(attr, "cfg") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("test") {
            found = true;
        }
        Ok(())
    });
    found
}

fn derive_idents(attr: &syn::Attribute) -> Vec<String> {
    if !attr_path_is(attr, "derive") {
        return Vec::new();
    }
    let mut names = Vec::new();
    let parsed = attr.parse_args_with(
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
    );
    if let Ok(paths) = parsed {
        for path in paths {
            if let Some(seg) = path.segments.last() {
                names.push(seg.ident.to_string());
            }
        }
    }
    names
}

fn is_violation(item_enum: &syn::ItemEnum) -> bool {
    if !matches!(item_enum.vis, syn::Visibility::Public(_)) {
        return false;
    }

    let mut has_non_exhaustive = false;
    let mut has_repr = false;
    let mut derives: Vec<String> = Vec::new();

    for attr in &item_enum.attrs {
        if attr_path_is(attr, "non_exhaustive") {
            has_non_exhaustive = true;
        }
        if attr_path_is(attr, "repr") {
            has_repr = true;
        }
        derives.extend(derive_idents(attr));
    }

    let is_error_type = derives.iter().any(|d| d == "Error");
    let is_serde_dto = derives
        .iter()
        .any(|d| d == "Serialize" || d == "Deserialize");

    is_error_type && !has_non_exhaustive && !has_repr && !is_serde_dto
}

fn enum_span_line(item_enum: &syn::ItemEnum) -> Option<usize> {
    let line = item_enum.ident.span().start().line;
    if line == 0 { None } else { Some(line) }
}

#[cfg(test)]
mod tests {
    use super::is_violation;

    fn first_enum(src: &str) -> syn::ItemEnum {
        let file = syn::parse_file(src).expect("test fixture must parse");
        for item in file.items {
            if let syn::Item::Enum(e) = item {
                return e;
            }
        }
        panic!("test fixture must contain an enum");
    }

    fn collect_from(src: &str) -> usize {
        let file = syn::parse_file(src).expect("test fixture must parse");
        let mut out = Vec::new();
        super::collect_enums(&file.items, &mut out);
        out.len()
    }

    #[test]
    fn skips_cfg_test_module_bodies() {
        let src =
            "#[cfg(test)] mod tests { #[derive(thiserror::Error)] pub enum TestOnlyError { A } }";
        assert_eq!(collect_from(src), 0);
    }

    #[test]
    fn collects_non_test_module_bodies() {
        let src = "mod inner { #[derive(thiserror::Error)] pub enum InnerError { A } }";
        assert_eq!(collect_from(src), 1);
    }

    #[test]
    fn positive_flags_bare_error_enum() {
        let e = first_enum("#[derive(Debug, thiserror::Error)] pub enum FooError { A }");
        assert!(is_violation(&e));
    }

    #[test]
    fn negative_compliant_non_exhaustive_first() {
        let e = first_enum(
            "#[non_exhaustive] #[derive(Debug, thiserror::Error)] pub enum FooError { A }",
        );
        assert!(!is_violation(&e));
    }

    #[test]
    fn negative_compliant_reversed_attr_order() {
        let e = first_enum("#[derive(thiserror::Error)] #[non_exhaustive] pub enum BarError { A }");
        assert!(!is_violation(&e));
    }

    #[test]
    fn negative_repr_dto() {
        let e =
            first_enum("#[repr(u8)] #[derive(Debug)] pub enum CollectionFailureReason { A = 0 }");
        assert!(!is_violation(&e));
    }

    #[test]
    fn negative_serde_dto() {
        let e = first_enum("#[derive(Serialize, Deserialize)] pub enum SchedulerEvent { A }");
        assert!(!is_violation(&e));
    }

    #[test]
    fn negative_serde_repr_dto() {
        let e = first_enum(
            "#[repr(u8)] #[derive(Serialize, Deserialize)] pub enum ExclusionReason { A = 0 }",
        );
        assert!(!is_violation(&e));
    }

    #[test]
    fn negative_non_pub_error_enum() {
        let e = first_enum("#[derive(thiserror::Error)] enum PrivErr { A }");
        assert!(!is_violation(&e));
    }
}
