use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::collections::BTreeMap;
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize, GenomeSafe)]
struct MyKey {
    id: u64,
}
#[derive(GenomeSafe)]
struct Container {
    data: BTreeMap<MyKey, String>,
}
fn main() {}
