use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

/// Derive macro recursively detects K inside Vec<BTreeMap<K, V>>
/// and adds GenomeOrd bound.
#[derive(GenomeSafe)]
struct Store<K> {
    buckets: Vec<BTreeMap<K, String>>,
}

fn main() {
    let _ = Store::<u32>::SCHEMA_HASH;
}
