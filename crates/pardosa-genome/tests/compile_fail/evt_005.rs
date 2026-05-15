//! EVT-005 — isize field rejected (platform-dependent size).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    n: isize,
}

fn main() {}
