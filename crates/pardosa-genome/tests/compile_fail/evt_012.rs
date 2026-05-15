//! EVT-012 — raw pointer field rejected (no canonical wire representation).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    p: *const u8,
}

fn main() {}
