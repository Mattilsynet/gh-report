//! EVT-013 — function pointer field rejected (process-local address; no wire form).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    f: fn(u32) -> u32,
}

fn main() {}
