use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::collections::HashSet;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    set: HashSet<u32>,
}
fn main() {}
