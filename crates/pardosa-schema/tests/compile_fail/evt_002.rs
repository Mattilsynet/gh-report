use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::collections::HashMap;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    map: HashMap<u32, u32>,
}
fn main() {}
