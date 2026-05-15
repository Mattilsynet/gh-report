//! EVT-008 — `#[serde(default)]` rejected (silently inert in genome format; ADR-029).
use pardosa_genome::GenomeSafe;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, GenomeSafe)]
#[serde(default)]
struct BadStruct {
    a: u32,
}

impl Default for BadStruct {
    fn default() -> Self {
        Self { a: 0 }
    }
}

fn main() {}
