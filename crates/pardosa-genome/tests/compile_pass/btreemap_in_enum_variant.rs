use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;
use serde::Serialize;

/// Derive macro detects K in BTreeMap inside an enum variant.
#[derive(GenomeSafe, Serialize)]
#[repr(u8)]
enum Container<K> {
    Empty = 0,
    WithMap { entries: BTreeMap<K, u32> } = 1,
}

fn main() {
    let _ = Container::<String>::SCHEMA_HASH;
}
