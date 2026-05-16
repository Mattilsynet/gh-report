//! EVT-013 backstop — function pointer behind a type alias.
//!
//! The syntactic EVT-013 check inspects the field type's syntactic
//! shape (`syn::Type::BareFn`). A type alias hides the bare-fn shape
//! behind a `Type::Path` last-segment ident, so EVT-013 does NOT fire.
//! The load-bearing backstop is `Serialize` (and `Encode`) trait
//! resolution: `fn(u32) -> u32` implements neither, so the derive
//! macros cannot satisfy their generated bounds.
//!
//! See `crates/pardosa-derive/src/lib.rs` top-level doc comment.
use pardosa_genome::GenomeSafe;
use serde::Serialize;

type Callback = fn(u32) -> u32;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    f: Callback,
}

fn main() {}
