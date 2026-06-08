use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::collections::HashMap;
type SafeMap = HashMap<u32, u32>;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    map: SafeMap,
}
fn main() {}
