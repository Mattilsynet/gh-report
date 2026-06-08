use pardosa_schema::GenomeSafe;
use serde::Serialize;
type Size = usize;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    n: Size,
}
fn main() {}
