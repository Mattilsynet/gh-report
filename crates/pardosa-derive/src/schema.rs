use quote::quote;
use syn::{Data, DeriveInput, Fields};
/// Build the canonical schema-source string for `input`.
///
/// Input: the parsed `DeriveInput`. Output: a human-readable canonical
/// rendering of the type's structural shape — `struct Name { field:
/// Type, ... }` or `enum Name { Variant { field: Type, ... }, ... }` —
/// with generic params rendered as `<T, U>` when present. The string is
/// embedded verbatim as `SCHEMA_SOURCE: &'static str` on the derived
/// `GenomeSafe` impl and shows up in diagnostics on schema-hash mismatch.
pub(crate) fn build_schema_source(input: &DeriveInput) -> syn::Result<String> {
    let name = &input.ident;
    let generics = if input.generics.params.is_empty() {
        String::new()
    } else {
        let params: Vec<String> = input
            .generics
            .type_params()
            .map(|tp| tp.ident.to_string())
            .collect();
        if params.is_empty() {
            String::new()
        } else {
            format!("<{}>", params.join(", "))
        }
    };
    match &input.data {
        Data::Struct(data) => {
            let fields = format_fields(&data.fields);
            Ok(format!("struct {name}{generics} {fields}"))
        }
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for variant in &data.variants {
                let vname = &variant.ident;
                let fields = format_fields(&variant.fields);
                if fields.is_empty() {
                    variants.push(format!("    {vname}"));
                } else {
                    variants.push(format!("    {vname}{fields}"));
                }
            }
            let body = variants.join(",\n");
            Ok(format!("enum {name}{generics} {{\n{body},\n}}"))
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "GenomeSafe cannot be derived for unions",
        )),
    }
}
/// Render a `Fields` shape as a canonical body string.
///
/// Input: `Fields` (named, unnamed, or unit). Output: ` { field:
/// Type, ... }` for named fields, `(Type, ...)` for tuple fields, and
/// the empty string for unit. Used by [`build_schema_source`] for both
/// struct and per-enum-variant bodies.
fn format_fields(fields: &Fields) -> String {
    match fields {
        Fields::Named(named) => {
            let entries: Vec<String> = named
                .named
                .iter()
                .map(|f| {
                    let fname = f.ident.as_ref().expect("named field");
                    let ftype = type_to_string(&f.ty);
                    format!("    {fname}: {ftype}")
                })
                .collect();
            if entries.is_empty() {
                " {}".to_string()
            } else {
                format!(" {{\n{},\n}}", entries.join(",\n"))
            }
        }
        Fields::Unnamed(unnamed) => {
            let entries: Vec<String> = unnamed
                .unnamed
                .iter()
                .map(|f| type_to_string(&f.ty))
                .collect();
            format!("({})", entries.join(", "))
        }
        Fields::Unit => String::new(),
    }
}
/// Render a `syn::Type` as a canonical type string.
///
/// Input: any `syn::Type`. Output: a `String` that strips module path
/// prefixes (keeping only the last segment of `Type::Path`) while
/// preserving generic arguments, reference / mutability, slice, array,
/// and tuple shapes. Two distinct paths sharing a final segment (e.g.
/// `std::vec::Vec` and `alloc::vec::Vec`) render identically — by
/// design, since `SCHEMA_SOURCE` documents shape, not the lookup path.
fn type_to_string(ty: &syn::Type) -> String {
    use syn::Type;
    match ty {
        Type::Path(tp) => {
            let segments: Vec<String> = tp
                .path
                .segments
                .iter()
                .map(|seg| {
                    let ident = seg.ident.to_string();
                    match &seg.arguments {
                        syn::PathArguments::None => ident,
                        syn::PathArguments::AngleBracketed(args) => {
                            let inner: Vec<String> = args
                                .args
                                .iter()
                                .map(|arg| match arg {
                                    syn::GenericArgument::Type(t) => type_to_string(t),
                                    other => quote!(# other).to_string(),
                                })
                                .collect();
                            format!("{ident}<{}>", inner.join(", "))
                        }
                        syn::PathArguments::Parenthesized(args) => {
                            let inner: Vec<String> =
                                args.inputs.iter().map(type_to_string).collect();
                            format!("{ident}({})", inner.join(", "))
                        }
                    }
                })
                .collect();
            segments.last().cloned().unwrap_or_default()
        }
        Type::Reference(r) => {
            let inner = type_to_string(&r.elem);
            if r.mutability.is_some() {
                format!("&mut {inner}")
            } else {
                format!("&{inner}")
            }
        }
        Type::Slice(s) => {
            let inner = type_to_string(&s.elem);
            format!("[{inner}]")
        }
        Type::Array(a) => {
            let inner = type_to_string(&a.elem);
            let len = &a.len;
            format!("[{inner}; {}]", quote!(# len))
        }
        Type::Tuple(t) => {
            let inner: Vec<String> = t.elems.iter().map(type_to_string).collect();
            format!("({})", inner.join(", "))
        }
        _ => quote!(# ty).to_string(),
    }
}
