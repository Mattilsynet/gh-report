use std::collections::BTreeSet;
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, GenomeSafe)]
struct MyItem {
    id: u64,
}

// MyItem is GenomeSafe + Ord but does NOT implement GenomeOrd.
// BTreeSet elements must implement GenomeOrd.
#[derive(GenomeSafe)]
struct Container {
    items: BTreeSet<MyItem>,
}

fn main() {}
