//! EVT-009 — `#[serde(tag = "...")]` rejected (internally tagged enums).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
#[serde(tag = "kind")]
#[repr(u8)]
enum BadEnum {
    A { x: u32 } = 0,
    B { y: u64 } = 1,
}

fn main() {}
