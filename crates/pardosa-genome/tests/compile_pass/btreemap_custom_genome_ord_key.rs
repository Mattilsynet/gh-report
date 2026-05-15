use std::collections::BTreeMap;
use pardosa_genome::{GenomeSafe, GenomeOrd};
use serde::Serialize;

/// Custom key type with both GenomeSafe and GenomeOrd — valid as BTreeMap key.
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, GenomeSafe)]
struct CustomKey {
    id: u64,
}

impl GenomeOrd for CustomKey {}

#[derive(GenomeSafe)]
struct Registry {
    data: BTreeMap<CustomKey, String>,
}

fn main() {
    let _ = Registry::SCHEMA_HASH;
}
