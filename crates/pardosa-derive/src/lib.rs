#![forbid(unsafe_code)]
mod codec;
mod generics;
mod hash;
mod path_resolution;
mod reject;
mod schema;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, parse_macro_input};
/// Entry point for `#[derive(GenomeSafe)]`.
///
/// Input: a `struct` or `#[repr(u8)] enum` with explicit discriminants and
/// no rejected serde attrs. Output: a coordinated bundle
/// of `Sealed`, `EventSafe`, `GenomeSafe`, `Encode`, and `Decode` impls
/// emitted in one `TokenStream`. Errors from the input-validation pass are
/// converted via `to_compile_error` so they surface as compile errors at
/// the call site rather than panics.
#[proc_macro_derive(GenomeSafe)]
pub fn derive_genome_safe(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match derive_genome_safe_impl(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
/// Build the coordinated impl bundle for [`derive_genome_safe`].
///
/// Validates serde-attr rejections, repr/discriminant invariants
/// (`validate_enum_repr_u8`), and unions (rejected with `EVT-001`).
/// Then builds the schema-source string ([`schema::build_schema_source`]),
/// the `SCHEMA_HASH` constant ([`hash::build_hash_expr`]), the codec
/// where-clause adjustments, and finally the `Sealed` / `EventSafe` /
/// `GenomeSafe` / `Encode` / `Decode` impls. Generic type parameters
/// gain `GenomeSafe` bounds; parameters used as `BTreeMap` / `BTreeSet`
/// keys additionally gain `GenomeOrd + Ord` bounds (see
/// [`generics::collect_btree_key_params`]).
fn derive_genome_safe_impl(input: &DeriveInput) -> syn::Result<TokenStream2> {
    reject::reject_serde_attrs(&input.attrs, "type")?;
    match &input.data {
        Data::Struct(data) => reject::reject_field_serde_attrs(&data.fields)?,
        Data::Enum(data) => {
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
    let schema_source = schema::build_schema_source(input)?;
    let hash_expr = hash::build_hash_expr(input)?;
    let genome_ord_params = generics::collect_btree_key_params(input);
    let schema_for_bounds = path_resolution::schema_path();
    let extra_bounds = input.generics.type_params().map(|tp| {
        let ident = &tp.ident;
        let schema = &schema_for_bounds;
        if genome_ord_params.contains(&ident.to_string()) {
            quote! {
                # ident : # schema::GenomeSafe + # schema::GenomeOrd +
                ::core::cmp::Ord
            }
        } else {
            quote! {
                # ident : # schema::GenomeSafe
            }
        }
    });
    let where_clause = if input.generics.type_params().next().is_some() {
        let existing = where_clause.map(|w| &w.predicates);
        quote! {
            where # (# extra_bounds,) * # existing
        }
    } else {
        quote! {
            # where_clause
        }
    };
    let encode_impl = codec::build_encode_impl(input);
    let decode_impl = codec::build_decode_impl(input);
    let schema = path_resolution::schema_path();
    Ok(quote! {
        impl # impl_generics # schema::sealed::Sealed for # name # ty_generics #
        where_clause {} impl # impl_generics # schema::EventSafe for # name #
        ty_generics # where_clause {} impl # impl_generics # schema::GenomeSafe for #
        name # ty_generics # where_clause { const SCHEMA_HASH : u128 = # hash_expr;
        const SCHEMA_SOURCE : &'static str = # schema_source; } # encode_impl #
        decode_impl
    })
}
