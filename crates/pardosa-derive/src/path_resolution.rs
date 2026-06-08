use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::Ident;
/// Convert a [`FoundCrate`] result into a syn `Ident` usable in a path.
///
/// Input: a lookup result from `proc_macro_crate::crate_name` plus a
/// fallback name (used for `FoundCrate::Itself`). Output: a `syn::Ident`
/// with hyphens replaced by underscores so it can sit inside a `::path`.
fn ident_from_crate_name(found: FoundCrate, fallback: &str) -> Ident {
    let name = match found {
        FoundCrate::Itself => fallback.to_string(),
        FoundCrate::Name(n) => n,
    };
    Ident::new(&name.replace('-', "_"), Span::call_site())
}
/// Path prefix for items hosted in `pardosa-schema` (e.g. `GenomeSafe`,
/// `EventSafe`, `schema_hash_bytes`). Resolution order: `pardosa` (via
/// `__derive_support`) → `pardosa-schema` direct.
pub(crate) fn schema_path() -> TokenStream2 {
    if let Ok(found) = crate_name("pardosa") {
        let id = ident_from_crate_name(found, "pardosa");
        return quote! {
            ::# id::__derive_support
        };
    }
    match crate_name("pardosa-schema") {
        Ok(found) => {
            let id = ident_from_crate_name(found, "pardosa_schema");
            quote! {
                ::# id
            }
        }
        Err(_) => syn::Error::new(
            Span::call_site(),
            "pardosa-derive: neither `pardosa` nor `pardosa-schema` is in scope; \
             add one to your `[dependencies]` to use this derive",
        )
        .to_compile_error(),
    }
}
/// Path prefix for items hosted in `pardosa-wire` (e.g. `Encode`, `Decode`,
/// `Decoder`, `DecodeError`). Resolution order: `pardosa` (via
/// `__derive_support`) → `pardosa-wire` direct.
pub(crate) fn wire_path() -> TokenStream2 {
    if let Ok(found) = crate_name("pardosa") {
        let id = ident_from_crate_name(found, "pardosa");
        return quote! {
            ::# id::__derive_support
        };
    }
    match crate_name("pardosa-wire") {
        Ok(found) => {
            let id = ident_from_crate_name(found, "pardosa_wire");
            quote! {
                ::# id
            }
        }
        Err(_) => syn::Error::new(
            Span::call_site(),
            "pardosa-derive: neither `pardosa` nor `pardosa-wire` is in scope; \
             add one to your `[dependencies]` to use this derive",
        )
        .to_compile_error(),
    }
}
