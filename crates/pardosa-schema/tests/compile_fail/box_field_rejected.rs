use pardosa_schema::GenomeSafe;
use serde::Serialize;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    field: Box<u32>,
}

fn main() {}
