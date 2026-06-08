use pardosa_schema::GenomeSafe;
use std::collections::HashSet;
#[derive(GenomeSafe)]
struct Bad {
    data: &'static HashSet<String>,
}
fn main() {}
