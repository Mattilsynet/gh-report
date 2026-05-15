//! EVT-006 — `#[serde(flatten)]` rejected (breaks fixed layout).
use pardosa_genome::GenomeSafe;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    a: u32,
    #[serde(flatten)]
    extra: BTreeMap<String, u32>,
}

fn main() {}
