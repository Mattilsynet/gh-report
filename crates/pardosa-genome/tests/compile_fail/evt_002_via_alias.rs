//! EVT-002 backstop — HashMap behind a type alias.
//!
//! The syntactic EVT-002 check inspects the last-segment ident of the
//! field type. A type alias hides `HashMap` behind a different ident
//! (`SafeMap` here), so EVT-002 does NOT fire. The load-bearing
//! backstop is trait resolution: the generated `where SafeMap: Encode`
//! bound fails because `HashMap` has no `Encode` impl in pardosa-encoding.
//!
//! See `crates/pardosa-derive/src/lib.rs` top-level doc comment.
use pardosa_genome::GenomeSafe;
use serde::Serialize;
use std::collections::HashMap;

type SafeMap = HashMap<u32, u32>;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    map: SafeMap,
}

fn main() {}
