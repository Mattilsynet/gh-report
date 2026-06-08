use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::collections::BTreeSet;
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, GenomeSafe)]
struct MyItem {
    id: u64,
}
#[derive(GenomeSafe)]
struct Container {
    items: BTreeSet<MyItem>,
}
fn main() {}
