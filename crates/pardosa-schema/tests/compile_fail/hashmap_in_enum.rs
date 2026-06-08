use pardosa_schema::GenomeSafe;
use std::collections::HashMap;
#[derive(GenomeSafe)]
enum Bad {
    Variant { data: HashMap<String, u32> },
}
fn main() {}
