use pardosa_schema::GenomeSafe;
use std::collections::HashSet;
#[derive(GenomeSafe)]
struct Bad {
    items: Option<HashSet<String>>,
}
fn main() {}
