//! Derive macro for the `GenomeSafe` trait.
//!
//! Generates `SCHEMA_HASH` (xxh3-128 fingerprint) and `SCHEMA_SOURCE`
//! (human-readable Rust type definition) from struct/enum declarations.
//!
//! # Compile-Time Rejections — EVT Diagnostic Catalog (GEN-0037)
//!
//! Each rejection is tagged with a stable `EVT-NNN` code emitted into the
//! compiler error so user tooling and tests can match on the code rather
//! than free-text. The catalog is growth-friendly: new codes append.
//!
//! ## Catalog status: best-effort syntactic diagnostics, not a security boundary
//!
//! The EVT-NNN checks operate on the surface syntax of field types
//! (`syn::Type::Path` last-segment ident matching). They are **convenience
//! diagnostics**: they catch the common case where a user writes
//! `HashMap<K, V>`, `usize`, or `fn(..) -> ..` literally as a field type
//! and surface a friendly, stable error code pointing at the precise
//! anti-pattern.
//!
//! They are **not** a security or correctness boundary. Type aliases
//! (`type SafeMap = HashMap<K, V>;`), associated types, and re-exports
//! that hide the offending type behind a different last-segment ident
//! will silently bypass these syntactic checks.
//!
//! The **load-bearing backstop** is trait resolution. The derive generates
//! `impl Encode for T where <each_field>: Encode` (and analogous clauses
//! for `Decode`, `EventSafe`, `GenomeSafe`). `HashMap`, `HashSet`, `usize`,
//! `isize`, raw pointers, and function pointers have no `Encode`/`Decode`
//! impls in `pardosa-encoding`, so any attempt to actually compile a
//! downstream user of such a type — whether the offending type appears
//! directly or behind an alias — fails at the trait-resolution stage with
//! an `E0277` "the trait bound `…: Encode` is not satisfied" error.
//!
//! The trybuild fixtures in `crates/pardosa-genome/tests/compile_fail/`
//! pin both paths: direct-use fixtures (`evt_002.rs`, `evt_004.rs`,
//! `evt_013.rs`, …) pin the EVT-NNN diagnostic; via-alias fixtures
//! (`evt_002_via_alias.rs`, `evt_004_via_alias.rs`, `evt_013_via_alias.rs`)
//! pin the trait-resolution backstop. Either failure path is acceptable;
//! what is **not** acceptable is a fixture passing compilation. Should
//! that happen, the offending type has reached an unsound code path and
//! the bead/ADR governing that EVT must be reopened.
//!
//! - `EVT-001` — union (unsupported data shape)
//! - `EVT-002` — `HashMap` field (non-deterministic iteration order)
//! - `EVT-003` — `HashSet` field (non-deterministic iteration order)
//! - `EVT-004` — `usize` field (platform-dependent size)
//! - `EVT-005` — `isize` field (platform-dependent size)
//! - `EVT-006` — `#[serde(flatten)]` (breaks fixed layout)
//! - `EVT-007` — `#[serde(untagged)]` (silently bypasses variant serialization)
//! - `EVT-008` — `#[serde(default)]` (inert; ADR-029)
//! - `EVT-009` — `#[serde(tag = "...")]` (internally tagged enum)
//! - `EVT-010` — `#[serde(content = "...")]` (adjacently tagged enum)
//! - `EVT-011` — `#[serde(skip_serializing_if = "...")]` (conditional omission)
//! - `EVT-012` — raw pointer field (`*const T` / `*mut T`)
//! - `EVT-013` — function pointer field (`fn(..) -> ..`)

mod schema;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Derive the `GenomeSafe` trait for a struct or enum.
///
/// Generates:
/// - `SCHEMA_HASH`: xxh3-128 of a canonical type representation (u128 per GEN-0035)
/// - `SCHEMA_SOURCE`: cleaned Rust source text for file header embedding
#[proc_macro_derive(GenomeSafe)]
pub fn derive_genome_safe(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_genome_safe_impl(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_genome_safe_impl(input: &DeriveInput) -> syn::Result<TokenStream2> {
    // --- Reject unsupported serde attributes (type-level and field-level) ---
    reject_serde_attrs(&input.attrs, "type")?;
    match &input.data {
        Data::Struct(data) => reject_field_serde_attrs(&data.fields)?,
        Data::Enum(data) => {
            // Well-formedness: enum must be `#[repr(u8)]` with explicit
            // discriminant literals on every variant (GEN-0035:R4). Checked
            // before per-variant field validation so the error surfaces
            // cleanly at the type ident.
            validate_enum_repr_u8(input, data)?;
            for variant in &data.variants {
                reject_serde_attrs(&variant.attrs, "variant")?;
                reject_field_serde_attrs(&variant.fields)?;
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "EVT-001: GenomeSafe cannot be derived for unions",
            ));
        }
    }

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // --- Build schema source string ---
    let schema_source = schema::build_schema_source(input)?;

    // --- Build schema hash expression ---
    let hash_expr = build_hash_expr(input)?;

    // --- Add GenomeSafe (+ GenomeOrd + Ord where needed) bounds to generic parameters ---
    // BTreeMap-key / BTreeSet-element params get `+ Ord` mechanically: the
    // F2 `EventSafe: Encode` supertrait (GEN-0037) makes `BTreeMap<K, _>:
    // EventSafe` resolve `Encode for BTreeMap<K, _>`, which requires
    // `K: Ord`. `GenomeOrd` is the user-asserted *deterministic* ordering;
    // `Ord` is the mechanical std requirement. Both must be present.
    let genome_ord_params = collect_btree_key_params(input);
    let extra_bounds = input.generics.type_params().map(|tp| {
        let ident = &tp.ident;
        if genome_ord_params.contains(&ident.to_string()) {
            quote! {
                #ident: ::pardosa_genome::GenomeSafe
                    + ::pardosa_genome::GenomeOrd
                    + ::core::cmp::Ord
            }
        } else {
            quote! { #ident: ::pardosa_genome::GenomeSafe }
        }
    });

    let where_clause = if input.generics.type_params().next().is_some() {
        let existing = where_clause.map(|w| &w.predicates);
        quote! { where #(#extra_bounds,)* #existing }
    } else {
        quote! { #where_clause }
    };

    // --- Build Encode/Decode impls (GEN-0035) ---
    let encode_impl = build_encode_impl(input);
    let decode_impl = build_decode_impl(input);

    Ok(quote! {
        impl #impl_generics ::pardosa_genome::sealed::Sealed for #name #ty_generics
            #where_clause
        {
        }

        impl #impl_generics ::pardosa_genome::EventSafe for #name #ty_generics
            #where_clause
        {
        }

        impl #impl_generics ::pardosa_genome::GenomeSafe for #name #ty_generics
            #where_clause
        {
            const SCHEMA_HASH: u128 = #hash_expr;
            const SCHEMA_SOURCE: &'static str = #schema_source;
        }

        #encode_impl
        #decode_impl
    })
}

// ---------------------------------------------------------------------------
// Schema hash computation
// ---------------------------------------------------------------------------

fn build_hash_expr(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name_str = input.ident.to_string();

    match &input.data {
        Data::Struct(data) => {
            let field_hash_exprs = build_field_hash_exprs(&data.fields);
            Ok(quote! {
                {
                    let mut h = ::pardosa_genome::schema_hash_bytes(
                        concat!("struct:", #name_str).as_bytes()
                    );
                    #(#field_hash_exprs)*
                    h
                }
            })
        }
        Data::Enum(data) => {
            let variant_exprs: Vec<TokenStream2> = data
                .variants
                .iter()
                .map(|v| {
                    let vname = v.ident.to_string();
                    let field_hashes = build_field_hash_exprs(&v.fields);
                    quote! {
                        h = ::pardosa_genome::schema_hash_combine(
                            h,
                            ::pardosa_genome::schema_hash_bytes(
                                concat!("variant:", #vname).as_bytes()
                            ),
                        );
                        #(#field_hashes)*
                    }
                })
                .collect();
            Ok(quote! {
                {
                    let mut h = ::pardosa_genome::schema_hash_bytes(
                        concat!("enum:", #name_str).as_bytes()
                    );
                    #(#variant_exprs)*
                    h
                }
            })
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "GenomeSafe cannot be derived for unions",
        )),
    }
}

fn build_field_hash_exprs(fields: &Fields) -> Vec<TokenStream2> {
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let fname = f.ident.as_ref().expect("named field").to_string();
                let fty = &f.ty;
                quote! {
                    h = ::pardosa_genome::schema_hash_combine(
                        h,
                        ::pardosa_genome::schema_hash_bytes(#fname.as_bytes()),
                    );
                    h = ::pardosa_genome::schema_hash_combine(
                        h,
                        <#fty as ::pardosa_genome::GenomeSafe>::SCHEMA_HASH,
                    );
                }
            })
            .collect(),
        Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .map(|f| {
                let fty = &f.ty;
                quote! {
                    h = ::pardosa_genome::schema_hash_combine(
                        h,
                        <#fty as ::pardosa_genome::GenomeSafe>::SCHEMA_HASH,
                    );
                }
            })
            .collect(),
        Fields::Unit => vec![],
    }
}

// ---------------------------------------------------------------------------
// BTreeMap/BTreeSet key parameter detection
// ---------------------------------------------------------------------------
//
// Walks field types to find generic type parameters used in BTreeMap key or
// BTreeSet element position. These parameters need GenomeOrd bounds in addition
// to GenomeSafe.
//
// Uses last-segment matching (e.g., `BTreeMap` not `std::collections::BTreeMap`).
// Known limitation: type aliases wrapping BTreeMap/BTreeSet are not detected.

/// Collect generic type parameter names that appear in `BTreeMap` key or `BTreeSet`
/// element position.
fn collect_btree_key_params(input: &DeriveInput) -> std::collections::HashSet<String> {
    let generic_names: std::collections::HashSet<String> = input
        .generics
        .type_params()
        .map(|tp| tp.ident.to_string())
        .collect();

    if generic_names.is_empty() {
        return std::collections::HashSet::new();
    }

    let mut result = std::collections::HashSet::new();

    let fields: Vec<&syn::Field> = match &input.data {
        Data::Struct(data) => iter_fields(&data.fields).collect(),
        Data::Enum(data) => data
            .variants
            .iter()
            .flat_map(|v| iter_fields(&v.fields))
            .collect(),
        Data::Union(_) => return result,
    };

    for field in fields {
        find_btree_key_params(&field.ty, &generic_names, &mut result);
    }

    result
}

/// Iterate over fields regardless of named/unnamed/unit variant.
fn iter_fields(fields: &Fields) -> Box<dyn Iterator<Item = &syn::Field> + '_> {
    match fields {
        Fields::Named(named) => Box::new(named.named.iter()),
        Fields::Unnamed(unnamed) => Box::new(unnamed.unnamed.iter()),
        Fields::Unit => Box::new(std::iter::empty()),
    }
}

/// Recursively walk a type looking for BTreeMap/BTreeSet usage.
/// When found, extract the key/element type and collect generic params from it.
fn find_btree_key_params(
    ty: &syn::Type,
    generics: &std::collections::HashSet<String>,
    result: &mut std::collections::HashSet<String>,
) {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if ident == "BTreeMap" {
                    if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                        // First type arg is the key — collect generic params from it.
                        if let Some(syn::GenericArgument::Type(key_ty)) = args.args.first() {
                            collect_generic_idents(key_ty, generics, result);
                        }
                        // Recurse into value type for nested BTreeMaps.
                        for arg in args.args.iter().skip(1) {
                            if let syn::GenericArgument::Type(inner) = arg {
                                find_btree_key_params(inner, generics, result);
                            }
                        }
                    }
                } else if ident == "BTreeSet" {
                    if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                        // First type arg is the element.
                        if let Some(syn::GenericArgument::Type(elem_ty)) = args.args.first() {
                            collect_generic_idents(elem_ty, generics, result);
                            // Recurse into element type for nested BTreeMap/BTreeSet.
                            find_btree_key_params(elem_ty, generics, result);
                        }
                    }
                } else {
                    // Recurse into type arguments (handles Vec<BTreeMap<K,V>>,
                    // Option<BTreeMap<K,V>>, Box<BTreeMap<K,V>>, etc.)
                    if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                        for arg in &args.args {
                            if let syn::GenericArgument::Type(inner) = arg {
                                find_btree_key_params(inner, generics, result);
                            }
                        }
                    }
                }
            }
        }
        Type::Reference(r) => find_btree_key_params(&r.elem, generics, result),
        Type::Slice(s) => find_btree_key_params(&s.elem, generics, result),
        Type::Array(a) => find_btree_key_params(&a.elem, generics, result),
        Type::Tuple(t) => {
            for elem in &t.elems {
                find_btree_key_params(elem, generics, result);
            }
        }
        Type::Paren(p) => find_btree_key_params(&p.elem, generics, result),
        _ => {}
    }
}

/// Recursively collect all generic type parameter identifiers from a type
/// expression. Used to extract params from `BTreeMap` key / `BTreeSet` element
/// position.
fn collect_generic_idents(
    ty: &syn::Type,
    generics: &std::collections::HashSet<String>,
    result: &mut std::collections::HashSet<String>,
) {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            // Bare generic ident (e.g., `K` in BTreeMap<K, V>).
            if tp.path.segments.len() == 1 {
                let seg = &tp.path.segments[0];
                if matches!(seg.arguments, syn::PathArguments::None) {
                    let name = seg.ident.to_string();
                    if generics.contains(&name) {
                        result.insert(name);
                        return;
                    }
                }
            }
            // Recurse into type arguments (e.g., composite keys like Option<K>).
            for seg in &tp.path.segments {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            collect_generic_idents(inner, generics, result);
                        }
                    }
                }
            }
        }
        Type::Tuple(t) => {
            for elem in &t.elems {
                collect_generic_idents(elem, generics, result);
            }
        }
        Type::Reference(r) => collect_generic_idents(&r.elem, generics, result),
        Type::Array(a) => collect_generic_idents(&a.elem, generics, result),
        Type::Paren(p) => collect_generic_idents(&p.elem, generics, result),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Serde attribute rejection
// ---------------------------------------------------------------------------

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
fn reject_serde_attrs(attrs: &[syn::Attribute], context: &str) -> syn::Result<()> {
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

// ---------------------------------------------------------------------------
// Field-level serde attribute and type checking
// ---------------------------------------------------------------------------

/// Reject unsupported serde attributes on individual fields.
fn reject_field_serde_attrs(fields: &Fields) -> syn::Result<()> {
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

// ---------------------------------------------------------------------------
// Enum well-formedness (GEN-0035:R4)
// ---------------------------------------------------------------------------
//
// GEN-0035:R4 mandates `[discriminant:u8]` enum encoding with explicit
// `repr(u8)` discriminants. This is a well-formedness prerequisite for
// emitting canonical Encode/Decode — not a curated catalog rejection, so
// the diagnostic is an un-coded syn::Error. May be promoted to EVT-014
// in a future sub-mission if user-friendly catalog framing earns its keep.

/// Validate an enum carries `#[repr(u8)]` and every variant has an explicit
/// discriminant literal expression.
fn validate_enum_repr_u8(input: &DeriveInput, data: &syn::DataEnum) -> syn::Result<()> {
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

// ---------------------------------------------------------------------------
// Encode / Decode body emission (GEN-0035)
// ---------------------------------------------------------------------------
//
// Struct emission:
//   encode → fields encoded back-to-back in declaration order (R3 / unit / tuple).
//   decode → fields decoded in the same order.
//
// Enum emission (well-formedness already verified):
//   encode → match on self, push the variant's explicit u8 discriminant, then
//            encode variant fields in declaration order.
//   decode → read 1 byte, match against the variant discriminants, decode
//            payload; unknown byte → EventError::InvalidInput.

fn build_encode_impl(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause_existing) = input.generics.split_for_impl();
    let where_clause = build_codec_where_clause(input, where_clause_existing, CodecMode::Encode);

    let body = match &input.data {
        Data::Struct(data) => build_struct_encode_body(&data.fields),
        Data::Enum(data) => build_enum_encode_body(data),
        Data::Union(_) => unreachable!("union rejected earlier"),
    };

    quote! {
        impl #impl_generics ::pardosa_encoding::Encode for #name #ty_generics
            #where_clause
        {
            fn encode(&self, out: &mut ::std::vec::Vec<u8>) {
                #body
            }
        }
    }
}

fn build_decode_impl(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause_existing) = input.generics.split_for_impl();
    let where_clause = build_codec_where_clause(input, where_clause_existing, CodecMode::Decode);

    let body = match &input.data {
        Data::Struct(data) => build_struct_decode_body(name, &data.fields),
        Data::Enum(data) => build_enum_decode_body(name, data),
        Data::Union(_) => unreachable!("union rejected earlier"),
    };

    quote! {
        impl #impl_generics ::pardosa_encoding::Decode for #name #ty_generics
            #where_clause
        {
            fn decode(
                d: &mut ::pardosa_encoding::Decoder<'_>,
            ) -> ::core::result::Result<Self, ::pardosa_encoding::EventError> {
                #body
            }
        }
    }
}

/// Build a where-clause for a codec impl.
///
/// `mode` selects which side of the codec is being emitted; only that side's
/// own bound is added to each generic type parameter. Mixing `Encode + Decode`
/// on both impls (the previous shape) leaked a spurious `T: Decode`
/// requirement into the `Encode` impl, which broke `EventSafe` resolution
/// under the F2 supertrait (GEN-0037).
///
/// Type parameters appearing in `BTreeMap` key position get an additional
/// `Ord` bound. On the Decode side they also get `Encode` because upstream
/// `impl<K: Decode + Encode + Ord, V: Decode> Decode for BTreeMap<K, V>`
/// requires it (the canonical-ordering check on decode re-encodes keys to
/// compare bytes).
fn build_codec_where_clause(
    input: &DeriveInput,
    existing: Option<&syn::WhereClause>,
    mode: CodecMode,
) -> TokenStream2 {
    let btree_key_params = collect_btree_key_params(input);
    let extra: Vec<TokenStream2> = input
        .generics
        .type_params()
        .map(|tp| {
            let ident = &tp.ident;
            match (mode, btree_key_params.contains(&ident.to_string())) {
                (CodecMode::Encode, false) => {
                    quote! { #ident: ::pardosa_encoding::Encode }
                }
                (CodecMode::Encode, true) => {
                    quote! { #ident: ::pardosa_encoding::Encode + ::core::cmp::Ord }
                }
                (CodecMode::Decode, false) => {
                    quote! { #ident: ::pardosa_encoding::Decode }
                }
                (CodecMode::Decode, true) => {
                    quote! {
                        #ident: ::pardosa_encoding::Decode
                            + ::pardosa_encoding::Encode
                            + ::core::cmp::Ord
                    }
                }
            }
        })
        .collect();

    if extra.is_empty() {
        return quote! { #existing };
    }

    let existing_preds = existing.map(|w| &w.predicates);
    quote! { where #(#extra,)* #existing_preds }
}

#[derive(Copy, Clone)]
enum CodecMode {
    Encode,
    Decode,
}

fn build_struct_encode_body(fields: &Fields) -> TokenStream2 {
    match fields {
        Fields::Named(named) => {
            let stmts = named.named.iter().map(|f| {
                let fname = f.ident.as_ref().expect("named field");
                quote! { ::pardosa_encoding::Encode::encode(&self.#fname, out); }
            });
            quote! { #(#stmts)* }
        }
        Fields::Unnamed(unnamed) => {
            let stmts = unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                let idx = syn::Index::from(i);
                quote! { ::pardosa_encoding::Encode::encode(&self.#idx, out); }
            });
            quote! { #(#stmts)* }
        }
        Fields::Unit => quote! {},
    }
}

fn build_struct_decode_body(name: &syn::Ident, fields: &Fields) -> TokenStream2 {
    match fields {
        Fields::Named(named) => {
            let inits = named.named.iter().map(|f| {
                let fname = f.ident.as_ref().expect("named field");
                let fty = &f.ty;
                quote! { #fname: <#fty as ::pardosa_encoding::Decode>::decode(d)? }
            });
            quote! { ::core::result::Result::Ok(#name { #(#inits,)* }) }
        }
        Fields::Unnamed(unnamed) => {
            let inits = unnamed.unnamed.iter().map(|f| {
                let fty = &f.ty;
                quote! { <#fty as ::pardosa_encoding::Decode>::decode(d)? }
            });
            quote! { ::core::result::Result::Ok(#name(#(#inits,)*)) }
        }
        Fields::Unit => quote! { ::core::result::Result::Ok(#name) },
    }
}

fn build_enum_encode_body(data: &syn::DataEnum) -> TokenStream2 {
    let arms = data.variants.iter().map(|v| {
        let vname = &v.ident;
        let Some((_, disc_lit)) = &v.discriminant else {
            unreachable!("validate_enum_repr_u8 enforced explicit discriminants")
        };
        match &v.fields {
            Fields::Named(named) => {
                let field_names: Vec<_> = named
                    .named
                    .iter()
                    .map(|f| f.ident.clone().expect("named field"))
                    .collect();
                quote! {
                    Self::#vname { #(#field_names),* } => {
                        out.push((#disc_lit) as u8);
                        #( ::pardosa_encoding::Encode::encode(#field_names, out); )*
                    }
                }
            }
            Fields::Unnamed(unnamed) => {
                let binds: Vec<syn::Ident> = (0..unnamed.unnamed.len())
                    .map(|i| syn::Ident::new(&format!("f{i}"), proc_macro2::Span::call_site()))
                    .collect();
                quote! {
                    Self::#vname(#(#binds),*) => {
                        out.push((#disc_lit) as u8);
                        #( ::pardosa_encoding::Encode::encode(#binds, out); )*
                    }
                }
            }
            Fields::Unit => quote! {
                Self::#vname => { out.push((#disc_lit) as u8); }
            },
        }
    });
    quote! {
        match self {
            #(#arms)*
        }
    }
}

fn build_enum_decode_body(name: &syn::Ident, data: &syn::DataEnum) -> TokenStream2 {
    let arms = data.variants.iter().map(|v| {
        let vname = &v.ident;
        let Some((_, disc_lit)) = &v.discriminant else {
            unreachable!("validate_enum_repr_u8 enforced explicit discriminants")
        };
        match &v.fields {
            Fields::Named(named) => {
                let inits = named.named.iter().map(|f| {
                    let fname = f.ident.as_ref().expect("named field");
                    let fty = &f.ty;
                    quote! { #fname: <#fty as ::pardosa_encoding::Decode>::decode(d)? }
                });
                quote! {
                    x if x == (#disc_lit) as u8 => {
                        ::core::result::Result::Ok(#name::#vname { #(#inits,)* })
                    }
                }
            }
            Fields::Unnamed(unnamed) => {
                let inits = unnamed.unnamed.iter().map(|f| {
                    let fty = &f.ty;
                    quote! { <#fty as ::pardosa_encoding::Decode>::decode(d)? }
                });
                quote! {
                    x if x == (#disc_lit) as u8 => {
                        ::core::result::Result::Ok(#name::#vname(#(#inits,)*))
                    }
                }
            }
            Fields::Unit => quote! {
                x if x == (#disc_lit) as u8 => {
                    ::core::result::Result::Ok(#name::#vname)
                }
            },
        }
    });
    quote! {
        let byte = ::pardosa_encoding::Decode::decode(d)?;
        let byte: u8 = byte;
        match byte {
            #(#arms)*
            _ => ::core::result::Result::Err(::pardosa_encoding::EventError::InvalidInput),
        }
    }
}
