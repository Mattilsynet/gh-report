use std::collections::HashMap;
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    data: HashMap<String, u32>,
}

fn main() {}
