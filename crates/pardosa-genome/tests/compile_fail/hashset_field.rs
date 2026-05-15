use std::collections::HashSet;
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    items: HashSet<String>,
}

fn main() {}
