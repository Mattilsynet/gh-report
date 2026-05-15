use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(GenomeSafe, Serialize)]
#[serde(rename = "MyPoint")]
struct Point {
    #[serde(rename = "x_coord")]
    x: f64,
    y: f64,
}

#[derive(GenomeSafe)]
struct Container {
    items: BTreeMap<String, u32>,
    values: Vec<Option<String>>,
}

fn main() {
    // Verify trait impls exist
    let _ = Point::SCHEMA_HASH;
    let _ = Container::SCHEMA_HASH;
}
