//! CI hard-gate: reject `#[non_exhaustive]` on public error enums.
//!
//! Run `--help` for usage and the exact FLAG-IFF heuristic (see [`HELP_TEXT`]).
//! Only literal `pub enum` tokens `syn` can parse are inspected; macro-generated
//! enums are invisible to this tool, so under-flagging is the deliberate safe
//! failure mode for this hard CI gate. Governed by RST-0006 / PGN-0006 / CHE-0021.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
const HELP_TEXT: &str =
    "non-exhaustive-check - CI hard-gate enforcing CLOSED error enums (C4.5/C4.6)

USAGE:
    non-exhaustive-check SOURCE_DIRECTORY...

Directories are required. tools/tripwires.sh non-exhaustive resolves library
targets from locked Cargo metadata, including external Cherry dependencies.

HEURISTIC (FLAG-IFF):
    An enum is a violation iff:
        is_error_type && has_non_exhaustive

    has_non_exhaustive = any attribute path is exactly `non_exhaustive`
    is_error_type      = any #[derive(..)] entry's last path segment is `Error`

LIMITS:
    Only literal pub enums are checked. Direct cfg predicates on enums and
    inline modules are skipped only when provably false with test=false and
    every other option unknown. all/any/not and Boolean literals are supported;
    parsing must consume the entire predicate. No feature environment is read.
    cfg_attr, macro expansion and out-of-line module ancestry are not evaluated.
    Error is a spelling heuristic: unrelated Error derives can match, aliases
    can be missed, and manual Error implementations are outside this rule.

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

    assert!(!args.is_empty(), "source directories are required");

    let mut crates_scanned = 0usize;
    let mut enums_scanned = 0usize;
    let mut violations: Vec<Violation> = Vec::new();

    for source in &args {
        let src_dir = PathBuf::from(source);
        assert!(src_dir.is_dir(), "missing source directory: {source}");
        crates_scanned += 1;

        let mut rs_files: Vec<PathBuf> = Vec::new();
        collect_rs_files(&src_dir, &mut rs_files);
        rs_files.sort();
        assert!(!rs_files.is_empty(), "no Rust sources: {source}");

        for file in rs_files {
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
            let parsed = syn::parse_file(&content)
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", file.display()));
            let mut enums: Vec<&syn::ItemEnum> = Vec::new();
            collect_enums(&parsed.items, &mut enums);

            for item_enum in enums {
                enums_scanned += 1;
                if is_violation(item_enum) {
                    let line = enum_span_line(item_enum);
                    violations.push(Violation {
                        path: file.clone(),
                        line,
                        enum_name: item_enum.ident.to_string(),
                    });
                }
            }
        }
    }

    if violations.is_empty() {
        println!(
            "OK: {crates_scanned} library crates scanned, {enums_scanned} pub enums, 0 violations (syntax-only predicate; enum count includes private declarations)"
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
            "VIOLATION\t{}\t{}\tforbidden #[non_exhaustive] on error enum (C4.5/C4.6 closed enumeration policy)",
            loc, v.enum_name
        );
    }
    println!(
        "SUMMARY: {crates_scanned} library crates scanned, {enums_scanned} pub enums, {} violations",
        violations.len()
    );
    std::process::exit(1);
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
                if m.attrs.iter().any(is_cfg_disabled_without_test) {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CfgTruth {
    False,
    True,
    Unknown,
}

impl syn::parse::Parse for CfgTruth {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        use syn::ext::IdentExt;

        if input.peek(syn::LitBool) {
            return Ok(if input.parse::<syn::LitBool>()?.value {
                Self::True
            } else {
                Self::False
            });
        }
        let name = input.call(syn::Ident::parse_any)?.unraw().to_string();
        if input.peek(syn::Token![=]) {
            input.parse::<syn::Token![=]>()?;
            input.parse::<syn::LitStr>()?;
            return Ok(Self::Unknown);
        }
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let values = content.parse_terminated(Self::parse, syn::Token![,])?;
            return match name.as_str() {
                "all" | "any" => {
                    let decisive = if name == "all" {
                        Self::False
                    } else {
                        Self::True
                    };
                    Ok(if values.iter().any(|value| *value == decisive) {
                        decisive
                    } else if values.iter().any(|value| *value == Self::Unknown) {
                        Self::Unknown
                    } else if name == "all" {
                        Self::True
                    } else {
                        Self::False
                    })
                }
                "not" if values.len() == 1 => Ok(match values[0] {
                    Self::False => Self::True,
                    Self::True => Self::False,
                    Self::Unknown => Self::Unknown,
                }),
                _ => Err(input.error("expected all(...), any(...) or not(one predicate)")),
            };
        }
        Ok(if name == "test" {
            Self::False
        } else {
            Self::Unknown
        })
    }
}

fn is_cfg_disabled_without_test(attr: &syn::Attribute) -> bool {
    attr_path_is(attr, "cfg")
        && matches!(
            attr.parse_args_with(|input: syn::parse::ParseStream<'_>| {
                let value = input.parse::<CfgTruth>()?;
                if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                }
                Ok(value)
            }),
            Ok(CfgTruth::False)
        )
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
    if !matches!(item_enum.vis, syn::Visibility::Public(_))
        || item_enum.attrs.iter().any(is_cfg_disabled_without_test)
    {
        return false;
    }

    let mut has_non_exhaustive = false;
    let mut derives: Vec<String> = Vec::new();

    for attr in &item_enum.attrs {
        if attr_path_is(attr, "non_exhaustive") {
            has_non_exhaustive = true;
        }
        derives.extend(derive_idents(attr));
    }

    let is_error_type = derives.iter().any(|d| d == "Error");

    is_error_type && has_non_exhaustive
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
    fn positive_flags_non_exhaustive_error_enum() {
        let e = first_enum(
            "#[non_exhaustive] #[derive(Debug, thiserror::Error)] pub enum FooError { A }",
        );
        assert!(is_violation(&e));
    }

    #[test]
    fn positive_flags_reversed_attr_order() {
        let e = first_enum("#[derive(thiserror::Error)] #[non_exhaustive] pub enum BarError { A }");
        assert!(is_violation(&e));
    }

    #[test]
    fn negative_compliant_closed_error_enum() {
        let e = first_enum("#[derive(Debug, thiserror::Error)] pub enum FooError { A }");
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
        let e = first_enum("#[derive(thiserror::Error)] #[non_exhaustive] enum PrivErr { A }");
        assert!(!is_violation(&e));
    }

    #[test]
    fn compound_cfg_test_only_is_skipped() {
        assert_eq!(
            collect_from(
                "#[cfg(all(test, feature = \"x\"))] mod tests { #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A } }"
            ),
            0
        );
    }

    #[test]
    fn cfg_truth_controls_enum_and_inline_module_scanning() {
        for (predicate, skipped) in [
            ("test", true),
            ("test,", true),
            ("r#test", true),
            ("not(test)", false),
            ("not(not(test))", true),
            ("all(test, feature = \"x\")", true),
            ("any(test, feature = \"x\")", false),
            ("not(any(not(test), feature = \"x\"))", true),
            ("all()", false),
            ("any()", true),
            ("true", false),
            ("false", true),
            ("feature = \"test\"", false),
            ("test = \"x\"", false),
            ("all(test, broken())", false),
            ("test, broken()", false),
            ("not(test, test)", false),
            ("not()", false),
            ("test = 1", false),
            ("test extra", false),
        ] {
            let attrs = format!("#[cfg({predicate})]");
            let declaration = "#[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }";
            let e = first_enum(&format!("{attrs} {declaration}"));
            assert_eq!(is_violation(&e), !skipped, "enum: {predicate}");
            assert_eq!(
                collect_from(&format!("{attrs} mod m {{ {declaration} }}")),
                usize::from(!skipped),
                "module: {predicate}"
            );
        }
    }

    #[test]
    fn cfg_attr_is_not_expanded_and_derive_identity_is_not_resolved() {
        for source in [
            "#[cfg_attr(not(test), cfg(test))] #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }",
            "#[derive(unrelated::Error)] #[non_exhaustive] pub enum E { A }",
        ] {
            assert!(is_violation(&first_enum(source)));
        }
        for source in [
            "#[cfg_attr(feature = \"x\", non_exhaustive)] #[derive(thiserror::Error)] pub enum E { A }",
            "#[derive(AliasedError)] #[non_exhaustive] pub enum E { A }",
            "#[derive(Debug)] #[non_exhaustive] pub enum E { A }",
            "#[derive(thiserror::Error)] #[non_exhaustive] pub(crate) enum E { A }",
            "#[cfg(feature = \"x\")] #[cfg(test)] #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }",
        ] {
            assert!(!is_violation(&first_enum(source)));
        }
        assert_eq!(
            collect_from("#[cfg_attr(not(test), cfg(test))] mod m { pub enum E { A } }"),
            1
        );
    }
}
