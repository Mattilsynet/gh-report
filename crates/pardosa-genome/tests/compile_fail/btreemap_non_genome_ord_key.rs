use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, GenomeSafe)]
struct MyKey {
    id: u64,
}

// MyKey is GenomeSafe + Ord but does NOT implement GenomeOrd.
// BTreeMap keys must implement GenomeOrd.
#[derive(GenomeSafe)]
struct Container {
    data: BTreeMap<MyKey, String>,
}

fn main() {}
