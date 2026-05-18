//! Encode / Decode codegen (GEN-0035).
//!
//! Struct emission:
//!   encode → fields encoded back-to-back in declaration order (R3 / unit / tuple).
//!   decode → fields decoded in the same order.
//!
//! Enum emission (well-formedness already verified):
//!   encode → match on self, push the variant's explicit u8 discriminant, then
//!            encode variant fields in declaration order.
//!   decode → read 1 byte, match against the variant discriminants, decode
//!            payload; unknown byte → EventError::InvalidInput.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DataEnum, DeriveInput, Fields};

pub(crate) fn build_encode_impl(input: &DeriveInput) -> TokenStream2 {
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

pub(crate) fn build_decode_impl(input: &DeriveInput) -> TokenStream2 {
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
    let btree_key_params = crate::generics::collect_btree_key_params(input);
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

fn build_enum_encode_body(data: &DataEnum) -> TokenStream2 {
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

fn build_enum_decode_body(name: &syn::Ident, data: &DataEnum) -> TokenStream2 {
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
