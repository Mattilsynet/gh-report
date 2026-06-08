use pardosa_schema::GenomeSafe;
use std::collections::BTreeMap;
#[derive(GenomeSafe)]
struct Bad {
    map: BTreeMap<u32, u32>,
}
fn main() {}
