//! EVT-001 — GenomeSafe cannot be derived for unions.
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
union BadUnion {
    a: u32,
    b: f32,
}

fn main() {}
