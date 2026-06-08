use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    n: usize,
}
fn main() {}
