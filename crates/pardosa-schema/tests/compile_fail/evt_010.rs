use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(Serialize, GenomeSafe)]
#[serde(tag = "k", content = "v")]
#[repr(u8)]
enum BadEnum {
    A(u32) = 0,
    B(u64) = 1,
}
fn main() {}
