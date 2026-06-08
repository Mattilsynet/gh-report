use pardosa_schema::GenomeSafe;
use serde::Serialize;
type Callback = fn(u32) -> u32;
#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    f: Callback,
}
fn main() {}
