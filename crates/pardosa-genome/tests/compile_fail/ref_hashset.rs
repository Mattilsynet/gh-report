use std::collections::HashSet;
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    data: &'static HashSet<String>,
}

fn main() {}
