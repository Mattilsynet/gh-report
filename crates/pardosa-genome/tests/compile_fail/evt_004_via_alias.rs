//! EVT-004 backstop — usize behind a type alias.
//!
//! The syntactic EVT-004 check inspects the last-segment ident of the
//! field type. A type alias hides `usize` behind a different ident
//! (`Size` here), so EVT-004 does NOT fire. The load-bearing backstop
//! is trait resolution: the generated `where Size: Encode` bound fails
//! because `usize` has no `Encode` impl in pardosa-encoding.
//!
//! See `crates/pardosa-derive/src/lib.rs` top-level doc comment.
use pardosa_genome::GenomeSafe;
use serde::Serialize;

type Size = usize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    n: Size,
}

fn main() {}
