//! Generic parameter analysis for `BTreeMap`/`BTreeSet` key bound inference.
//!
//! Walks field types to find generic type parameters used in `BTreeMap` key or
//! `BTreeSet` element position. These parameters need `GenomeOrd` bounds in
//! addition to `GenomeSafe`.
//!
//! Uses last-segment matching (e.g., `BTreeMap` not
//! `std::collections::BTreeMap`). Known limitation: type aliases wrapping
//! `BTreeMap`/`BTreeSet` are not detected.

use std::collections::HashSet;
use syn::{Data, DeriveInput, Fields};

/// Collect generic type parameter names that appear in `BTreeMap` key or `BTreeSet`
/// element position.
pub(crate) fn collect_btree_key_params(input: &DeriveInput) -> HashSet<String> {
    let generic_names: HashSet<String> = input
        .generics
        .type_params()
        .map(|tp| tp.ident.to_string())
        .collect();

    if generic_names.is_empty() {
        return HashSet::new();
    }

    let mut result = HashSet::new();

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
    generics: &HashSet<String>,
    result: &mut HashSet<String>,
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
    generics: &HashSet<String>,
    result: &mut HashSet<String>,
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
