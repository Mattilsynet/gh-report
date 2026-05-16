//! syn-based parsing: collect line ranges of `#[cfg(test)]`-gated items.

use syn::spanned::Spanned;
use syn::{Attribute, File, Item};

/// Return inclusive `(start_line, end_line)` ranges for every top-level or
/// nested item annotated with a cfg expression containing `test`.
///
/// Errors propagate as syn parse errors.
pub fn test_ranges_for_file(source: &str) -> Result<Vec<(usize, usize)>, syn::Error> {
    let file: File = syn::parse_file(source)?;
    let mut ranges = Vec::new();
    for item in &file.items {
        collect_from_item(item, &mut ranges);
    }
    Ok(ranges)
}

fn collect_from_item(item: &Item, ranges: &mut Vec<(usize, usize)>) {
    let attrs = item_attrs(item);
    if attrs_have_test_cfg(attrs) {
        let span = item.span();
        let start = span.start().line;
        let end = span.end().line;
        ranges.push((start, end));
        // No need to recurse — outer range subsumes any nested test cfgs.
        return;
    }
    // Otherwise, recurse into nested modules with inline content.
    if let Item::Mod(m) = item
        && let Some((_, items)) = &m.content
    {
        for child in items {
            collect_from_item(child, ranges);
        }
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// True iff any attribute is `#[cfg(...)]` whose argument tokens contain
/// the bare identifier `test`.
#[must_use]
pub fn attrs_have_test_cfg(attrs: &[Attribute]) -> bool {
    attrs.iter().any(attr_is_test_cfg)
}

fn attr_is_test_cfg(attr: &Attribute) -> bool {
    // Match #[cfg(...)] only — not #[cfg_attr(...)], not other paths.
    if !attr.path().is_ident("cfg") {
        return false;
    }
    // syn::Meta::List with the args as a token stream.
    let list = match &attr.meta {
        syn::Meta::List(l) => l,
        _ => return false,
    };
    tokens_contain_test_ident(&list.tokens.to_string())
}

/// Heuristic: does the stringified token stream contain `test` as a
/// standalone identifier (word boundary on both sides)?
///
/// Avoids false-positives like `target_test` (hypothetical) or `"test"`
/// inside a string literal feature key.
#[must_use]
pub fn tokens_contain_test_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    let needle = b"test";
    let n = needle.len();
    let mut i = 0usize;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle {
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after_ok = i + n == bytes.len() || !is_ident_byte(bytes[i + n]);
            if before_ok && after_ok {
                // Reject if inside a string literal — crude heuristic: count
                // unescaped quotes before this position; odd = inside string.
                if !inside_string_literal(bytes, i) {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn inside_string_literal(bytes: &[u8], pos: usize) -> bool {
    let mut in_str = false;
    let mut i = 0;
    while i < pos {
        let b = bytes[i];
        if b == b'\\' && in_str {
            i += 2;
            continue;
        }
        if b == b'"' {
            in_str = !in_str;
        }
        i += 1;
    }
    in_str
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn cfg_test_detected() {
        let attr: Attribute = parse_quote!(#[cfg(test)]);
        assert!(attr_is_test_cfg(&attr));
    }

    #[test]
    fn cfg_any_test_foo_detected() {
        let attr: Attribute = parse_quote!(#[cfg(any(test, foo))]);
        assert!(attr_is_test_cfg(&attr));
    }

    #[test]
    fn cfg_all_test_foo_detected() {
        let attr: Attribute = parse_quote!(#[cfg(all(test, foo))]);
        assert!(attr_is_test_cfg(&attr));
    }

    #[test]
    fn cfg_target_os_not_detected() {
        let attr: Attribute = parse_quote!(#[cfg(target_os = "linux")]);
        assert!(!attr_is_test_cfg(&attr));
    }

    #[test]
    fn cfg_feature_with_test_string_not_detected() {
        // feature = "test" — the bare ident is not `test`; it's a string.
        let attr: Attribute = parse_quote!(#[cfg(feature = "test")]);
        assert!(!attr_is_test_cfg(&attr));
    }

    #[test]
    fn cfg_attr_not_a_cfg_gate() {
        let attr: Attribute = parse_quote!(#[cfg_attr(test, derive(Debug))]);
        // We deliberately only match #[cfg(...)], not #[cfg_attr(...)].
        assert!(!attr_is_test_cfg(&attr));
    }

    #[test]
    fn nested_test_mod_collected() {
        let src = "fn alpha() {}\n\
                   \n\
                   #[cfg(test)]\n\
                   mod tests {\n\
                       fn t1() {}\n\
                   }\n\
                   \n\
                   fn beta() {}\n";
        let ranges = test_ranges_for_file(src).expect("parse ok");
        assert_eq!(ranges.len(), 1, "exactly one test range");
        let (s, e) = ranges[0];
        // The `#[cfg(test)]` attr is line 3; closing brace line 6.
        assert!(
            s <= 3 && e >= 6,
            "range should span attr through closing brace: got ({s}, {e})"
        );
    }

    #[test]
    fn nested_inside_nontest_mod_collected() {
        let src = "mod outer {\n\
                       fn a() {}\n\
                       #[cfg(test)]\n\
                       mod inner_tests {\n\
                           fn t() {}\n\
                       }\n\
                       fn b() {}\n\
                   }\n";
        let ranges = test_ranges_for_file(src).expect("parse ok");
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn no_test_cfg_no_ranges() {
        let src = "fn a() {}\nfn b() {}\n";
        let ranges = test_ranges_for_file(src).expect("parse ok");
        assert!(ranges.is_empty());
    }
}
