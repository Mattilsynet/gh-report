use pardosa_schema::GenomeSafe;
use serde::Serialize;
#[derive(GenomeSafe, Serialize)]
struct Metrics {
    #[serde(rename = "flatten_count")]
    count: u32,
    #[serde(rename = "untagged_value")]
    value: u64,
}
fn main() {
    let _ = Metrics::SCHEMA_HASH;
}
