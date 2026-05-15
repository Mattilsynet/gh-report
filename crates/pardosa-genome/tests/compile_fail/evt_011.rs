//! EVT-011 — `#[serde(skip_serializing_if = "...")]` rejected (conditional omission).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    #[serde(skip_serializing_if = "Option::is_none")]
    a: Option<u32>,
}

fn main() {}
