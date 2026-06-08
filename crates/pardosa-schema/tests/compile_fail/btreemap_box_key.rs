use pardosa_schema::GenomeSafe;
use std::collections::BTreeMap;
#[derive(GenomeSafe)]
struct Container {
    data: BTreeMap<Box<String>, u32>,
}
fn main() {}
