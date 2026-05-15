use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

/// Derive macro auto-detects that K is used as a BTreeMap key
/// and adds GenomeOrd bound alongside GenomeSafe.
#[derive(GenomeSafe)]
struct Indexed<K> {
    entries: BTreeMap<K, u32>,
}

fn main() {
    let _ = Indexed::<String>::SCHEMA_HASH;
}
