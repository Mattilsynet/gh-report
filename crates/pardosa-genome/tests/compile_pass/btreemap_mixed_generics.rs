use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

/// K gets GenomeOrd (used as map key), V and T get only GenomeSafe.
#[derive(GenomeSafe)]
struct Mixed<K, V, T> {
    index: BTreeMap<K, V>,
    items: Vec<T>,
}

fn main() {
    let _ = Mixed::<u32, String, f64>::SCHEMA_HASH;
}
