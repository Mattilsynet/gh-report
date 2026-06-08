use crate::path_resolution;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields};
/// Build the `SCHEMA_HASH: u128` initialiser expression for `input`.
///
/// Returns a `TokenStream` that, at the call site, hashes the type's
/// structural shape — kind prefix (`struct:` / `enum:`), name, then
/// per-variant or per-field folds — into a `u128` via
/// `schema_hash_bytes` and `schema_hash_combine` from `pardosa_schema`.
/// Byte-identical shapes hash identically; renaming a field or
/// reordering variants changes the hash (the schema-mismatch gate from
/// ADR-0005 + ADR-0006).
pub(crate) fn build_hash_expr(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name_str = input.ident.to_string();
    let schema = path_resolution::schema_path();
    match &input.data {
        Data::Struct(data) => {
            let field_hash_exprs = build_field_hash_exprs(&data.fields);
            Ok(quote! {
                { let mut h = # schema::schema_hash_bytes(concat!("struct:", #
                name_str) .as_bytes()); # (# field_hash_exprs) * h }
            })
        }
        Data::Enum(data) => {
            let variant_exprs: Vec<TokenStream2> = data
                .variants
                .iter()
                .map(|v| {
                    let vname = v.ident.to_string();
                    let field_hashes = build_field_hash_exprs(&v.fields);
                    let disc_lit = v
                        .discriminant
                        .as_ref()
                        .map(|(_, expr)| expr)
                        .expect("validate_enum_repr_u8 ensures explicit discriminant");
                    quote! {
                        h = # schema::schema_hash_combine(h, #
                        schema::schema_hash_bytes(concat!("variant:", # vname)
                        .as_bytes()),); h = # schema::schema_hash_combine(h, #
                        schema::schema_hash_bytes(& [(# disc_lit) as u8],),); # (#
                        field_hashes) *
                    }
                })
                .collect();
            Ok(quote! {
                { let mut h = # schema::schema_hash_bytes(concat!("enum:", #
                name_str) .as_bytes()); # (# variant_exprs) * h }
            })
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "GenomeSafe cannot be derived for unions",
        )),
    }
}
/// Build per-field hash-fold expressions for [`build_hash_expr`].
///
/// Input: `Fields` (named, unnamed, or unit). Output: per-field token
/// streams that fold the field name (named fields only) and the field
/// type's own `SCHEMA_HASH` into the accumulator `h`. Unit fields produce
/// no expressions.
fn build_field_hash_exprs(fields: &Fields) -> Vec<TokenStream2> {
    let schema = path_resolution::schema_path();
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let fname = f.ident.as_ref().expect("named field").to_string();
                let fty = &f.ty;
                let s = &schema;
                quote! {
                    h = # s::schema_hash_combine(h, # s::schema_hash_bytes(# fname
                    .as_bytes()),); h = # s::schema_hash_combine(h, <# fty as #
                    s::GenomeSafe >::SCHEMA_HASH,);
                }
            })
            .collect(),
        Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .map(|f| {
                let fty = &f.ty;
                let s = &schema;
                quote! {
                    h = # s::schema_hash_combine(h, <# fty as # s::GenomeSafe
                    >::SCHEMA_HASH,);
                }
            })
            .collect(),
        Fields::Unit => vec![],
    }
}
