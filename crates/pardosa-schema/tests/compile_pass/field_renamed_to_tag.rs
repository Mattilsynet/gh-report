use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(GenomeSafe, Serialize)]
struct Config {
    #[serde(rename = "tag")]
    label: u32,
    #[serde(rename = "skip_serializing_if")]
    flag: bool,
}
fn main() {
    let _ = Config::SCHEMA_HASH;
}
