//! Compile-time rejections — EVT diagnostic implementations (GEN-0037).
//!
//! Implementation of the EVT-NNN syntactic rejections. The canonical catalog
//! (status, scope, backstop) lives in the crate-level `//!` docs of `lib.rs`;
//! this module holds only the implementations.

use syn::{Attribute, DataEnum, DeriveInput, Fields};

/// Rejected serde attribute keys that break fixed-layout serialization.
/// Tuple format: (attr-name, EVT-code, reason).
const REJECTED_PATH_ATTRS: &[(&str, &str, &str)] = &[
    (
        "flatten",
        "EVT-006",
        "Flattening causes serde to emit serialize_map instead of \
         serialize_struct, breaking fixed-layout serialization.",
    ),
    (
        "untagged",
        "EVT-007",
        "Untagged enums bypass variant serialization, causing silent \
         data corruption in fixed-layout formats.",
    ),
    (
        "default",
        "EVT-008",
        "#[serde(default)] is silently inert in genome format because \
         all fields are always present on the wire. Rejected to prevent \
         misleading annotations (ADR-029).",
    ),
];

const REJECTED_NAME_VALUE_ATTRS: &[(&str, &str, &str)] = &[
    (
        "tag",
        "EVT-009",
        "Only externally tagged enums (serde default) are compatible \
         with fixed discriminant-based layout.",
    ),
    (
        "skip_serializing_if",
        "EVT-011",
        "Conditional field omission breaks fixed-layout serialization.",
    ),
    (
        "content",
        "EVT-010",
        "Adjacently tagged enums (tag + content) are not compatible \
         with fixed discriminant-based layout. Use externally tagged \
         enums (serde default).",
    ),
];

/// Check attributes for unsupported serde modifiers using structured
/// `syn::Meta` parsing (not string matching) to avoid false positives
/// on rename values or field names containing rejection keywords.
pub(crate) fn reject_serde_attrs(attrs: &[Attribute], context: &str) -> syn::Result<()> {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            let ident_str = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();

            // Check path-only attrs: #[serde(flatten)], #[serde(untagged)]
            for &(key, code, reason) in REJECTED_PATH_ATTRS {
                if ident_str == key {
                    return Err(syn::Error::new_spanned(
                        &meta.path,
                        format!(
                            "{code}: GenomeSafe: #[serde({key})] is not supported on {context}. \
                             {reason}"
                        ),
                    ));
                }
            }

            // Check name-value attrs: #[serde(tag = "...")], #[serde(skip_serializing_if = "...")]
            for &(key, code, reason) in REJECTED_NAME_VALUE_ATTRS {
                if ident_str == key {
                    return Err(syn::Error::new_spanned(
                        &meta.path,
                        format!(
                            "{code}: GenomeSafe: #[serde({key} = \"...\")] is not supported on \
                             {context}. {reason}"
                        ),
                    ));
                }
            }

            // Skip value tokens for allowed attrs (rename = "...", etc.)
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Token![=] = meta.input.parse()?;
                let _: syn::Lit = meta.input.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let _content;
                syn::parenthesized!(_content in meta.input);
            }

            Ok(())
        })?;
    }

    Ok(())
}

/// Reject unsupported serde attributes on individual fields.
pub(crate) fn reject_field_serde_attrs(fields: &Fields) -> syn::Result<()> {
    let field_iter: Box<dyn Iterator<Item = &syn::Field>> = match fields {
        Fields::Named(named) => Box::new(named.named.iter()),
        Fields::Unnamed(unnamed) => Box::new(unnamed.unnamed.iter()),
        Fields::Unit => return Ok(()),
    };

    for field in field_iter {
        reject_serde_attrs(&field.attrs, "field")?;
        reject_hashmap_type(&field.ty)?;
        reject_platform_sized_type(&field.ty)?;
        reject_pointer_type(&field.ty)?;
    }
    Ok(())
}

/// Reject `HashMap` and `HashSet` types, recursing into all type positions.
fn reject_hashmap_type(ty: &syn::Type) -> syn::Result<()> {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if ident == "HashMap" {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "EVT-002: GenomeSafe: HashMap has non-deterministic iteration order. \
                         Use BTreeMap for deterministic serialization.",
                    ));
                }
                if ident == "HashSet" {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "EVT-003: GenomeSafe: HashSet has non-deterministic iteration order. \
                         Use BTreeSet for deterministic serialization.",
                    ));
                }
            }
            // Check generic arguments recursively
            if let Some(last) = tp.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &last.arguments
            {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(inner) = arg {
                        reject_hashmap_type(inner)?;
                    }
                }
            }
        }
        Type::Reference(r) => reject_hashmap_type(&r.elem)?,
        Type::Slice(s) => reject_hashmap_type(&s.elem)?,
        Type::Array(a) => reject_hashmap_type(&a.elem)?,
        Type::Tuple(t) => {
            for elem in &t.elems {
                reject_hashmap_type(elem)?;
            }
        }
        Type::Paren(p) => reject_hashmap_type(&p.elem)?,
        _ => {}
    }
    Ok(())
}

/// Reject `usize` and `isize` types, recursing into all type positions.
///
/// These types have platform-dependent size (32-bit on 32-bit targets,
/// 64-bit on 64-bit targets), which breaks cross-platform schema
/// compatibility and deterministic serialization.
fn reject_platform_sized_type(ty: &syn::Type) -> syn::Result<()> {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if ident == "usize" {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "EVT-004: GenomeSafe: usize has platform-dependent size. \
                         Use u32/u64 for portable serialization.",
                    ));
                }
                if ident == "isize" {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "EVT-005: GenomeSafe: isize has platform-dependent size. \
                         Use i32/i64 for portable serialization.",
                    ));
                }
            }
            // Check generic arguments recursively (catches Vec<usize>, Option<isize>, etc.)
            if let Some(last) = tp.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &last.arguments
            {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(inner) = arg {
                        reject_platform_sized_type(inner)?;
                    }
                }
            }
        }
        Type::Reference(r) => reject_platform_sized_type(&r.elem)?,
        Type::Slice(s) => reject_platform_sized_type(&s.elem)?,
        Type::Array(a) => reject_platform_sized_type(&a.elem)?,
        Type::Tuple(t) => {
            for elem in &t.elems {
                reject_platform_sized_type(elem)?;
            }
        }
        Type::Paren(p) => reject_platform_sized_type(&p.elem)?,
        _ => {}
    }
    Ok(())
}

/// Reject raw-pointer (`*const T`, `*mut T`) and function-pointer
/// (`fn(..) -> ..`) types in any field position.
///
/// - Raw pointers carry no ownership and no canonical wire representation;
///   emitted as EVT-012.
/// - Function pointers are address values that have no portable wire form;
///   emitted as EVT-013.
///
/// Recurses through references, slices, arrays, tuples, paren-wrappers, and
/// angle-bracketed generic arguments so e.g. `Option<*const u8>` and
/// `Vec<fn() -> u32>` are caught at field position.
fn reject_pointer_type(ty: &syn::Type) -> syn::Result<()> {
    use syn::Type;
    match ty {
        Type::Ptr(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "EVT-012: GenomeSafe: raw pointers (*const T / *mut T) have no \
                 canonical wire representation. Use Box<T>, &T, or an explicit \
                 owned value instead.",
            ));
        }
        Type::BareFn(_) => {
            return Err(syn::Error::new_spanned(
                ty,
                "EVT-013: GenomeSafe: function pointers (fn(..) -> ..) carry \
                 process-local addresses with no portable wire representation. \
                 Encode the action as data (e.g. an enum tag) instead.",
            ));
        }
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &last.arguments
            {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(inner) = arg {
                        reject_pointer_type(inner)?;
                    }
                }
            }
        }
        Type::Reference(r) => reject_pointer_type(&r.elem)?,
        Type::Slice(s) => reject_pointer_type(&s.elem)?,
        Type::Array(a) => reject_pointer_type(&a.elem)?,
        Type::Tuple(t) => {
            for elem in &t.elems {
                reject_pointer_type(elem)?;
            }
        }
        Type::Paren(p) => reject_pointer_type(&p.elem)?,
        _ => {}
    }
    Ok(())
}

// Enum well-formedness (GEN-0035:R4)
//
// GEN-0035:R4 mandates `[discriminant:u8]` enum encoding with explicit
// `repr(u8)` discriminants. This is a well-formedness prerequisite for
// emitting canonical Encode/Decode — not a curated catalog rejection, so
// the diagnostic is an un-coded syn::Error. May be promoted to EVT-014
// in a future sub-mission if user-friendly catalog framing earns its keep.

/// Validate an enum carries `#[repr(u8)]` and every variant has an explicit
/// discriminant literal expression.
pub(crate) fn validate_enum_repr_u8(input: &DeriveInput, data: &DataEnum) -> syn::Result<()> {
    // 1. The enum must be `#[repr(u8)]`.
    let has_repr_u8 = input.attrs.iter().any(|a| {
        if !a.path().is_ident("repr") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("u8") {
                found = true;
            }
            // Skip any value/paren payload so the parser doesn't error.
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Token![=] = meta.input.parse()?;
                let _: syn::Lit = meta.input.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let _content;
                syn::parenthesized!(_content in meta.input);
            }
            Ok(())
        });
        found
    });

    if !has_repr_u8 {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "GenomeSafe: enum must be #[repr(u8)] for canonical encoding \
             (GEN-0035:R4). Annotate the enum with `#[repr(u8)]` and give each \
             variant an explicit discriminant literal (e.g. `Variant = 0`).",
        ));
    }

    // 2. Every variant must have an explicit discriminant `= <integer-literal>`.
    for variant in &data.variants {
        match &variant.discriminant {
            Some((_, expr)) => {
                // Accept integer literals only; reject paths / consts / exprs
                // so the discriminant is statically a u8 byte at derive time.
                if !matches!(
                    expr,
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Int(_),
                        ..
                    })
                ) {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "GenomeSafe: variant discriminant must be a u8 integer \
                         literal for canonical encoding (GEN-0035:R4). \
                         Replace with an explicit literal such as `= 7`.",
                    ));
                }
            }
            None => {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    "GenomeSafe: every enum variant must carry an explicit \
                     discriminant literal for canonical encoding (GEN-0035:R4). \
                     Add `= <u8>` to this variant (e.g. `Variant = 0`).",
                ));
            }
        }
    }

    Ok(())
}
