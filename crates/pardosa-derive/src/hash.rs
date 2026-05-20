//! Schema hash computation — xxh3-128 fingerprint expression generation.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub(crate) fn build_hash_expr(input: &DeriveInput) -> syn::Result<TokenStream2> {
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
                    // GEN-0035:R9 — fold the explicit repr(u8) discriminant
                    // *value* into SCHEMA_HASH alongside the variant name.
                    // The wire byte (GEN-0035:R4) is this explicit value, not
                    // the 0-indexed position; the schema fingerprint must
                    // mirror the wire so two enums identical in shape but
                    // differing in discriminant assignment hash distinctly.
                    // `reject.rs::validate_enum_repr_u8` guarantees
                    // `v.discriminant` is `Some((_, ExprLit{Lit::Int, ..}))`
                    // by the time we get here.
                    let disc_lit = v
                        .discriminant
                        .as_ref()
                        .map(|(_, expr)| expr)
                        .expect("validate_enum_repr_u8 ensures explicit discriminant");
                    quote! {
                        h = ::pardosa_genome::schema_hash_combine(
                            h,
                            ::pardosa_genome::schema_hash_bytes(
                                concat!("variant:", #vname).as_bytes()
                            ),
                        );
                        h = ::pardosa_genome::schema_hash_combine(
                            h,
                            ::pardosa_genome::schema_hash_bytes(
                                &[(#disc_lit) as u8],
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
