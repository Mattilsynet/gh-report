use pardosa_schema::GenomeSafe;
use std::collections::BTreeMap;
use std::collections::HashMap;
#[derive(GenomeSafe)]
struct Bad {
    data: BTreeMap<String, HashMap<String, u32>>,
}
fn main() {}
