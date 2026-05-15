//! EVT-007 — `#[serde(untagged)]` rejected (silently bypasses variant tagging).
use pardosa_genome::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
#[serde(untagged)]
#[repr(u8)]
enum BadEnum {
    A(u32) = 0,
    B(u64) = 1,
}

fn main() {}
