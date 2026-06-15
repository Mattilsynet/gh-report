use pardosa_schema::GenomeSafe;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize, GenomeSafe)]
struct BadStruct {
    field: Arc<u32>,
}

fn main() {}
