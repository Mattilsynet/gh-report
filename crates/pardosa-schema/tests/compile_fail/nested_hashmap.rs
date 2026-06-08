use pardosa_schema::GenomeSafe;
use std::collections::HashMap;
#[derive(GenomeSafe)]
struct Bad {
    data: Vec<HashMap<String, u32>>,
}
fn main() {}
