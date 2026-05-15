use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

// Box<String> is GenomeSafe but does NOT implement GenomeOrd.
// Map keys must be owned value types, not smart-pointer wrappers.
#[derive(GenomeSafe)]
struct Container {
    data: BTreeMap<Box<String>, u32>,
}

fn main() {}
