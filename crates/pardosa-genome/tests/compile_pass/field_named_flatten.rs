use pardosa_genome::GenomeSafe;
use serde::Serialize;

/// A field with "flatten" in its serde rename value must NOT be rejected.
/// Only the serde key `flatten` (as in `#[serde(flatten)]`) is forbidden,
/// not rename values that happen to contain the word.
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
