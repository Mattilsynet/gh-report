use std::collections::HashSet;
use syn::{Data, DeriveInput, Fields};
/// Collect type-param names that appear as `BTreeMap` or `BTreeSet` keys.
///
/// Input: the parsed `DeriveInput`. Output: a `HashSet<String>` of
/// generic-type-param identifiers used in key positions of any
/// `BTreeMap<K, V>` / `BTreeSet<K>` field anywhere in the type. These
/// params need an extra `GenomeOrd + Ord` (encode) or `Encode + Ord`
/// (decode) bound on the synthesised where-clause so canonical
/// ordering is preserved across round-trips.
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
/// Iterate fields uniformly across struct field shapes.
///
/// Input: a `Fields` value. Output: a boxed iterator yielding each
/// underlying `syn::Field`, with `Fields::Unit` producing an empty
/// iterator. Used internally to flatten variant / struct walks.
fn iter_fields(fields: &Fields) -> Box<dyn Iterator<Item = &syn::Field> + '_> {
    match fields {
        Fields::Named(named) => Box::new(named.named.iter()),
        Fields::Unnamed(unnamed) => Box::new(unnamed.unnamed.iter()),
        Fields::Unit => Box::new(std::iter::empty()),
    }
}
/// Recursively walk a type looking for generic params used as `BTreeMap`
/// / `BTreeSet` keys.
///
/// Input: a `syn::Type`, the set of generic-param names in scope, and a
/// mutable accumulator. Output: writes into `result` any generic-param
/// idents found in key positions, descending through references, slices,
/// arrays, tuples, and generic arguments of arbitrary path types.
fn find_btree_key_params(ty: &syn::Type, generics: &HashSet<String>, result: &mut HashSet<String>) {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            if let Some(last) = tp.path.segments.last() {
                let ident = last.ident.to_string();
                if ident == "BTreeMap" {
                    if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                        if let Some(syn::GenericArgument::Type(key_ty)) = args.args.first() {
                            collect_generic_idents(key_ty, generics, result);
                        }
                        for arg in args.args.iter().skip(1) {
                            if let syn::GenericArgument::Type(inner) = arg {
                                find_btree_key_params(inner, generics, result);
                            }
                        }
                    }
                } else if ident == "BTreeSet"
                    && let syn::PathArguments::AngleBracketed(args) = &last.arguments
                {
                    if let Some(syn::GenericArgument::Type(elem_ty)) = args.args.first() {
                        collect_generic_idents(elem_ty, generics, result);
                        find_btree_key_params(elem_ty, generics, result);
                    }
                } else if let syn::PathArguments::AngleBracketed(args) = &last.arguments {
                    for arg in &args.args {
                        if let syn::GenericArgument::Type(inner) = arg {
                            find_btree_key_params(inner, generics, result);
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
/// Collect generic-param idents that appear as bare names in a type
/// position.
///
/// Input: a `syn::Type`, the set of generic-param names in scope, and a
/// mutable accumulator. Output: writes into `result` any single-segment
/// path matching a known generic-param ident; does not recurse into
/// `BTreeMap` / `BTreeSet` key arms specifically (that is
/// [`find_btree_key_params`]'s job).
fn collect_generic_idents(
    ty: &syn::Type,
    generics: &HashSet<String>,
    result: &mut HashSet<String>,
) {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
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
