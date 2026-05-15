//! EVT-003 — HashSet field rejected (non-deterministic iteration order).
use pardosa_genome::GenomeSafe;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    set: HashSet<u32>,
}

fn main() {}
