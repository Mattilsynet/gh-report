use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    f: fn(u32) -> u32,
}
fn main() {}
