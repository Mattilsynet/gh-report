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

mod codec;
mod generics;
mod hash;
mod reject;
mod schema;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, parse_macro_input};

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
    reject::reject_serde_attrs(&input.attrs, "type")?;
    match &input.data {
        Data::Struct(data) => reject::reject_field_serde_attrs(&data.fields)?,
        Data::Enum(data) => {
            // Well-formedness: enum must be `#[repr(u8)]` with explicit
            // discriminant literals on every variant (GEN-0035:R4). Checked
            // before per-variant field validation so the error surfaces
            // cleanly at the type ident.
            reject::validate_enum_repr_u8(input, data)?;
            for variant in &data.variants {
                reject::reject_serde_attrs(&variant.attrs, "variant")?;
                reject::reject_field_serde_attrs(&variant.fields)?;
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
    let hash_expr = hash::build_hash_expr(input)?;

    // --- Add GenomeSafe (+ GenomeOrd + Ord where needed) bounds to generic parameters ---
    // BTreeMap-key / BTreeSet-element params get `+ Ord` mechanically: the
    // F2 `EventSafe: Encode` supertrait (GEN-0037) makes `BTreeMap<K, _>:
    // EventSafe` resolve `Encode for BTreeMap<K, _>`, which requires
    // `K: Ord`. `GenomeOrd` is the user-asserted *deterministic* ordering;
    // `Ord` is the mechanical std requirement. Both must be present.
    let genome_ord_params = generics::collect_btree_key_params(input);
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
    let encode_impl = codec::build_encode_impl(input);
    let decode_impl = codec::build_decode_impl(input);

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
