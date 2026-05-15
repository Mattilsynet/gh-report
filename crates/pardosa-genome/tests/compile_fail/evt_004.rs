//! EVT-004 — usize field rejected (platform-dependent size).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    n: usize,
}

fn main() {}
