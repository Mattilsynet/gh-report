//! EVT-002 — HashMap field rejected (non-deterministic iteration order).
use pardosa_genome::GenomeSafe;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    map: HashMap<u32, u32>,
}

fn main() {}
