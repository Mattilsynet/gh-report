use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

/// Concrete tuple key: (u32, String) implements GenomeOrd.
#[derive(GenomeSafe)]
struct Registry {
    data: BTreeMap<(u32, String), Vec<u8>>,
}

fn main() {
    let _ = Registry::SCHEMA_HASH;
}
