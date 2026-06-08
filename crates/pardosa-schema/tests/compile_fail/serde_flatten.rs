use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(Serialize)]
struct Inner {
    x: u32,
}
#[derive(GenomeSafe)]
struct Bad {
    #[serde(flatten)]
    inner: Inner,
}
fn main() {}
