use crate::path_resolution;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DataEnum, DeriveInput, Fields};
/// Emit the `Encode` impl for `input`.
///
/// Input: the parsed `DeriveInput` (struct or enum; unions are unreachable
/// — rejected in [`crate::derive_genome_safe_impl`]). Output: an
/// `impl Encode for <T>` block that walks fields (struct) or variant
/// payloads + discriminant byte (enum), calling `Encode::encode` on each
/// field in declaration order. Generic params gain an `Encode` bound;
/// `BTreeMap` / `BTreeSet` key params additionally gain `Ord`.
pub(crate) fn build_encode_impl(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause_existing) = input.generics.split_for_impl();
    let wire = path_resolution::wire_path();
    let where_clause =
        build_codec_where_clause(input, where_clause_existing, CodecMode::Encode, &wire);
    let body = match &input.data {
        Data::Struct(data) => build_struct_encode_body(&data.fields, &wire),
        Data::Enum(data) => build_enum_encode_body(data, &wire),
        Data::Union(_) => unreachable!("union rejected earlier"),
    };
    quote! {
        impl # impl_generics # wire::Encode for # name # ty_generics # where_clause { fn
        encode(& self, out : & mut ::std::vec::Vec < u8 >) { # body } }
    }
}
/// Emit the `Decode` impl for `input`.
///
/// Input: the parsed `DeriveInput`. Output: an `impl Decode for <T>` block
/// that reverses [`build_encode_impl`]: structs decode fields in
/// declaration order; enums first decode a `u8` discriminant byte and
/// dispatch by matching against the variant's explicit discriminant
/// literal, returning `DecodeError::TagOutOfRange` on no match. Generic
/// params gain a `Decode` bound; `BTreeMap` / `BTreeSet` key params
/// additionally gain `Encode + Ord` so they can be re-keyed on decode.
pub(crate) fn build_decode_impl(input: &DeriveInput) -> TokenStream2 {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause_existing) = input.generics.split_for_impl();
    let wire = path_resolution::wire_path();
    let where_clause =
        build_codec_where_clause(input, where_clause_existing, CodecMode::Decode, &wire);
    let body = match &input.data {
        Data::Struct(data) => build_struct_decode_body(name, &data.fields, &wire),
        Data::Enum(data) => build_enum_decode_body(name, data, &wire),
        Data::Union(_) => unreachable!("union rejected earlier"),
    };
    quote! {
        impl # impl_generics # wire::Decode for # name # ty_generics # where_clause { fn
        decode(d : & mut # wire::Decoder <'_ >,) -> ::core::result::Result < Self, #
        wire::DecodeError > { # body } }
    }
}
/// Build the codec where-clause for `input`, mode-aware.
///
/// Input: existing where-clause (preserved verbatim) + the codec mode
/// (`Encode` or `Decode`). Output: a synthesised where-clause that adds
/// per-generic-param bounds — `Encode` or `Decode` for every type param,
/// plus `Ord` (encode) or `Encode + Ord` (decode) for type params that
/// appear as `BTreeMap` / `BTreeSet` keys (see
/// [`crate::generics::collect_btree_key_params`]).
fn build_codec_where_clause(
    input: &DeriveInput,
    existing: Option<&syn::WhereClause>,
    mode: CodecMode,
    wire: &TokenStream2,
) -> TokenStream2 {
    let btree_key_params = crate::generics::collect_btree_key_params(input);
    let extra: Vec<TokenStream2> = input
        .generics
        .type_params()
        .map(|tp| {
            let ident = &tp.ident;
            match (mode, btree_key_params.contains(&ident.to_string())) {
                (CodecMode::Encode, false) => {
                    quote! {
                        # ident : # wire::Encode
                    }
                }
                (CodecMode::Encode, true) => {
                    quote! {
                        # ident : # wire::Encode + ::core::cmp::Ord
                    }
                }
                (CodecMode::Decode, false) => {
                    quote! {
                        # ident : # wire::Decode
                    }
                }
                (CodecMode::Decode, true) => {
                    quote! {
                        # ident : # wire::Decode + # wire::Encode + ::core::cmp::Ord
                    }
                }
            }
        })
        .collect();
    if extra.is_empty() {
        return quote! {
            # existing
        };
    }
    let existing_preds = existing.map(|w| &w.predicates);
    quote! {
        where # (# extra,) * # existing_preds
    }
}
#[derive(Copy, Clone)]
enum CodecMode {
    Encode,
    Decode,
}
/// Build the encode body for a struct.
///
/// Input: struct field shape. Output: a sequence of
/// `Encode::encode(&self.<field>, out);` statements in declaration order
/// (named, unnamed, or empty for unit structs).
fn build_struct_encode_body(fields: &Fields, wire: &TokenStream2) -> TokenStream2 {
    match fields {
        Fields::Named(named) => {
            let stmts = named.named.iter().map(|f| {
                let fname = f.ident.as_ref().expect("named field");
                quote! {
                    # wire::Encode::encode(& self.# fname, out);
                }
            });
            quote! {
                # (# stmts) *
            }
        }
        Fields::Unnamed(unnamed) => {
            let stmts = unnamed.unnamed.iter().enumerate().map(|(i, _)| {
                let idx = syn::Index::from(i);
                quote! {
                    # wire::Encode::encode(& self.# idx, out);
                }
            });
            quote! {
                # (# stmts) *
            }
        }
        Fields::Unit => quote! {},
    }
}
/// Build the decode body for a struct.
///
/// Input: struct field shape. Output: `Ok(<Name> { <fields decoded in
/// declaration order via Decode::decode> })` (or the tuple / unit
/// equivalent). Each field decode propagates errors with `?`.
fn build_struct_decode_body(
    name: &syn::Ident,
    fields: &Fields,
    wire: &TokenStream2,
) -> TokenStream2 {
    match fields {
        Fields::Named(named) => {
            let inits = named.named.iter().map(|f| {
                let fname = f.ident.as_ref().expect("named field");
                let fty = &f.ty;
                quote! {
                    # fname : <# fty as # wire::Decode >::decode(d) ?
                }
            });
            quote! {
                ::core::result::Result::Ok(# name { # (# inits,) * })
            }
        }
        Fields::Unnamed(unnamed) => {
            let inits = unnamed.unnamed.iter().map(|f| {
                let fty = &f.ty;
                quote! {
                    <# fty as # wire::Decode >::decode(d) ?
                }
            });
            quote! {
                ::core::result::Result::Ok(# name(# (# inits,) *))
            }
        }
        Fields::Unit => {
            quote! {
                ::core::result::Result::Ok(# name)
            }
        }
    }
}
/// Build the encode body for an enum.
///
/// Input: enum variants with explicit `u8` discriminants (enforced by
/// [`crate::reject::validate_enum_repr_u8`]). Output: a `match self`
/// dispatch where each arm pushes the variant's discriminant byte then
/// encodes the variant's fields in declaration order.
fn build_enum_encode_body(data: &DataEnum, wire: &TokenStream2) -> TokenStream2 {
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
                    Self::# vname { # (# field_names),* } => { out.push((# disc_lit)
                    as u8); # (# wire::Encode::encode(# field_names, out);) * }
                }
            }
            Fields::Unnamed(unnamed) => {
                let binds: Vec<syn::Ident> = (0..unnamed.unnamed.len())
                    .map(|i| syn::Ident::new(&format!("f{i}"), proc_macro2::Span::call_site()))
                    .collect();
                quote! {
                    Self::# vname(# (# binds),*) => { out.push((# disc_lit) as u8); #
                    (# wire::Encode::encode(# binds, out);) * }
                }
            }
            Fields::Unit => {
                quote! {
                    Self::# vname => { out.push((# disc_lit) as u8); }
                }
            }
        }
    });
    quote! {
        match self { # (# arms) * }
    }
}
/// Build the decode body for an enum.
///
/// Input: enum variants. Output: a body that first decodes a `u8`
/// discriminant byte, then dispatches via `match` arms keyed on each
/// variant's discriminant literal. Unknown discriminants surface as
/// `DecodeError::TagOutOfRange { tag }`, preserving the original byte
/// in the diagnostic.
fn build_enum_decode_body(name: &syn::Ident, data: &DataEnum, wire: &TokenStream2) -> TokenStream2 {
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
                    quote! {
                        # fname : <# fty as # wire::Decode >::decode(d) ?
                    }
                });
                quote! {
                    x if x == (# disc_lit) as u8 => { ::core::result::Result::Ok(#
                    name::# vname { # (# inits,) * }) }
                }
            }
            Fields::Unnamed(unnamed) => {
                let inits = unnamed.unnamed.iter().map(|f| {
                    let fty = &f.ty;
                    quote! {
                        <# fty as # wire::Decode >::decode(d) ?
                    }
                });
                quote! {
                    x if x == (# disc_lit) as u8 => { ::core::result::Result::Ok(#
                    name::# vname(# (# inits,) *)) }
                }
            }
            Fields::Unit => {
                quote! {
                    x if x == (# disc_lit) as u8 => { ::core::result::Result::Ok(#
                    name::# vname) }
                }
            }
        }
    });
    quote! {
        let byte = # wire::Decode::decode(d) ?; let byte : u8 = byte; match byte { # (#
        arms) * tag => ::core::result::Result::Err(# wire::DecodeError::TagOutOfRange {
        tag : ::core::convert::From::from(tag) }), }
    }
}
