use syn::{Attribute, DataEnum, DeriveInput, Fields};
/// Serde attributes rejected in path form (`#[serde(flatten)]` etc.).
///
/// Entries are `(attribute_name, EVT-NNN code, human-readable reason)`.
/// Driven by [`reject_serde_attrs`] at type, variant, and field scope.
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
/// Serde attributes rejected in name-value form (`#[serde(tag = "...")]`).
///
/// Same shape as [`REJECTED_PATH_ATTRS`]: `(name, EVT-NNN code, reason)`.
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
/// Reject any rejected serde attribute on `attrs`.
///
/// Input: a slice of attributes plus a `context` string used in the
/// diagnostic ("type", "variant", "field"). Output: `Err(syn::Error)`
/// on the first rejected attribute encountered, with the EVT-NNN code
/// and reason quoted verbatim from [`REJECTED_PATH_ATTRS`] or
/// [`REJECTED_NAME_VALUE_ATTRS`]; otherwise `Ok(())`.
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
/// Reject serde attrs and forbidden types on every field in `fields`.
///
/// Input: a `Fields` value. Output: `Err(syn::Error)` on the first
/// rejected field-level serde attribute or first forbidden type
/// (`HashMap`, `BTreeMap`, raw `String` / `Vec<u8>`, `usize`, raw
/// pointers, etc.); otherwise `Ok(())`. The forbidden-type checks
/// surface as EVT-NNN diagnostics from the per-kind helpers below.
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
        reject_forbidden_without_wrapper(&field.ty)?;
    }
    Ok(())
}
/// Reject `HashMap` / `HashSet` / `BTreeMap` / `BTreeSet` field types.
///
/// Input: a `syn::Type`. Output: `Err` with EVT-014 if the type or any
/// nested generic argument names one of the forbidden map / set
/// containers; otherwise `Ok(())`. Recursion mirrors the other
/// reject_* helpers (references, slices, arrays, tuples, parens, and
/// generic args of arbitrary path types).
fn reject_hashmap_type(ty: &syn::Type) -> syn::Result<()> {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if matches!(
                    ident.as_str(),
                    "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet"
                ) {
                    return Err(syn::Error::new_spanned(
                        ty,
                        "EVT-014: GenomeSafe: Map- and set-shaped event payloads are \
                         forbidden by Solon doctrine (STORY §4.2): enumerate keys as \
                         named struct fields or an enum, or store the map outside the \
                         event store.",
                    ));
                }
            }
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
/// Reject `usize` / `isize` field types.
///
/// Input: a `syn::Type`. Output: `Err` with EVT-004 (`usize`) or
/// EVT-005 (`isize`) if the type contains a platform-sized integer at
/// any nesting depth; otherwise `Ok(())`. Rationale: `usize` / `isize`
/// are platform-dependent in size, so a wire format that names them
/// would silently break across 32 ↔ 64-bit boundaries.
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
/// Reject raw pointer (`*const T`, `*mut T`) and bare-function-pointer
/// (`fn(..) -> ..`) field types.
///
/// Input: a `syn::Type`. Output: `Err` with EVT-012 (raw pointer) or
/// EVT-013 (bare fn) if the type contains either at any nesting depth;
/// otherwise `Ok(())`. Rationale: process-local addresses have no
/// portable wire representation.
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
/// Look up rejection guidance for a syntactic ident, returning `None` for idents that are
/// not on the EVT-014 forbidden list. Split out of `reject_forbidden_without_wrapper`
/// to keep the per-type-shape dispatch short.
fn forbidden_ident_guidance(
    ident: &str,
    last: &syn::PathSegment,
    tp: &syn::TypePath,
) -> Option<&'static str> {
    match ident {
        "String" => Some(
            "Use `EventString<MAX>` or `NonEmptyEventString<MAX>` from \
             pardosa_schema::bounded; raw `String` is unbounded and refused.",
        ),
        "f32" => Some(
            "Use `OrderedF32` / `RealF32` from \
             pardosa_schema::floats per your NaN/Inf policy, or `EventF32` \
             when the payload must round-trip NaN/±Inf as event tags; raw \
             `f32` lacks a canonical total order.",
        ),
        "f64" => Some(
            "Use `OrderedF64` / `RealF64` from \
             pardosa_schema::floats per your NaN/Inf policy, or `EventF64` \
             when the payload must round-trip NaN/±Inf as event tags; raw \
             `f64` lacks a canonical total order.",
        ),
        "Cow" => Some(
            "Lifetimes are not persistable; use the owned bounded \
             wrapper (`EventString<MAX>` for text, `EventBytes<MAX>` for \
             bytes, owned `T` otherwise).",
        ),
        "Vec" => {
            if let syn::PathArguments::AngleBracketed(args) = &last.arguments
                && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
                && is_last_ident(inner, "u8")
            {
                Some(
                    "Use `EventBytes<MAX>` from pardosa_schema::bounded for \
                     byte payloads; raw `Vec<u8>` is unbounded and refused.",
                )
            } else {
                Some(
                    "Use `EventVec<T, MAX>` from pardosa_schema::bounded; \
                     raw `Vec<T>` is unbounded and refused.",
                )
            }
        }
        "str" if tp.path.segments.len() == 1 => Some(
            "Use the owned bounded wrapper (`EventString<MAX>` / \
             `NonEmptyEventString<MAX>`); `str` is unsized and references \
             are not persistable.",
        ),
        _ => None,
    }
}
/// Reject types that must be wrapped in a bounded `pardosa_schema`
/// wrapper.
///
/// Input: a `syn::Type`. Output: `Err` with EVT-014 if the type names a
/// bare `String`, `f32`, `f64`, `Cow`, `Vec` (any element type), `str`,
/// `&str`, `&[u8]`, or `[u8]` at any nesting depth; otherwise `Ok(())`.
/// Per-ident guidance comes from [`forbidden_ident_guidance`]; the
/// rejection points the user at the corresponding bounded wrapper
/// (`EventString<MAX>`, `EventBytes<MAX>`, `EventVec<T, MAX>`,
/// `OrderedF32` / `OrderedF64`, etc.).
fn reject_forbidden_without_wrapper(ty: &syn::Type) -> syn::Result<()> {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if let Some(advice) = forbidden_ident_guidance(&ident, last, tp) {
                    return Err(syn::Error::new_spanned(
                        ty,
                        format!("EVT-014: GenomeSafe: {advice}"),
                    ));
                }
            }
            if let Some(last) = tp.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &last.arguments
            {
                for arg in &args.args {
                    if let syn::GenericArgument::Type(inner) = arg {
                        reject_forbidden_without_wrapper(inner)?;
                    }
                }
            }
        }
        Type::Reference(r) => match &*r.elem {
            syn::Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "str") => {
                return Err(syn::Error::new_spanned(
                    ty,
                    "EVT-014: GenomeSafe: Use the owned bounded wrapper \
                         (`EventString<MAX>` / `NonEmptyEventString<MAX>`); `&str` \
                         is a reference and is not persistable.",
                ));
            }
            syn::Type::Slice(s) if is_last_ident(&s.elem, "u8") => {
                return Err(syn::Error::new_spanned(
                    ty,
                    "EVT-014: GenomeSafe: Use `EventBytes<MAX>` from \
                         pardosa_schema::bounded; `&[u8]` is a reference and is not \
                         persistable.",
                ));
            }
            _ => reject_forbidden_without_wrapper(&r.elem)?,
        },
        Type::Slice(s) => {
            if is_last_ident(&s.elem, "u8") {
                return Err(syn::Error::new_spanned(
                    ty,
                    "EVT-014: GenomeSafe: Use `EventBytes<MAX>` from \
                     pardosa_schema::bounded; `[u8]` is unsized and not persistable \
                     directly.",
                ));
            }
            reject_forbidden_without_wrapper(&s.elem)?;
        }
        Type::Array(a) => reject_forbidden_without_wrapper(&a.elem)?,
        Type::Tuple(t) => {
            for elem in &t.elems {
                reject_forbidden_without_wrapper(elem)?;
            }
        }
        Type::Paren(p) => reject_forbidden_without_wrapper(&p.elem)?,
        _ => {}
    }
    Ok(())
}
/// Return whether `ty` is a single-segment path whose last identifier
/// equals `name`. Cheap structural check used by the reject_* helpers
/// to dispatch on element-type shape (e.g. `Vec<u8>` vs `Vec<T>`).
fn is_last_ident(ty: &syn::Type, name: &str) -> bool {
    if let syn::Type::Path(tp) = ty
        && let Some(last) = tp.path.segments.last()
    {
        return last.ident == name;
    }
    false
}
/// Validate that an enum is `#[repr(u8)]` with explicit literal
/// discriminants on every variant.
///
/// Input: the `DeriveInput` (for the type-level `#[repr]`) plus the
/// `DataEnum`. Output: `Ok(())` if the enum carries `#[repr(u8)]` and
/// every variant has a discriminant of the form `= <integer literal>`;
/// otherwise `Err(syn::Error)` keyed on GEN-0035:R4. Rationale: canonical
/// enum encoding emits the discriminant as a single byte; without
/// `#[repr(u8)]` + explicit literals, inserting a variant could
/// silently renumber and break decoders of older bytes.
pub(crate) fn validate_enum_repr_u8(input: &DeriveInput, data: &DataEnum) -> syn::Result<()> {
    let has_repr_u8 = input.attrs.iter().any(|a| {
        if !a.path().is_ident("repr") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("u8") {
                found = true;
            }
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
    for variant in &data.variants {
        match &variant.discriminant {
            Some((_, expr)) => {
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
