use pardosa_genome::GenomeSafe;
use serde::Serialize;

/// A field renamed to "tag" must NOT be rejected.
/// Only the serde key `tag` (as in `#[serde(tag = "...")]`) is forbidden,
/// not rename values that happen to be the word "tag".
#[derive(GenomeSafe, Serialize)]
struct Config {
    #[serde(rename = "tag")]
    label: String,
    #[serde(rename = "skip_serializing_if")]
    flag: bool,
}

fn main() {
    let _ = Config::SCHEMA_HASH;
}
