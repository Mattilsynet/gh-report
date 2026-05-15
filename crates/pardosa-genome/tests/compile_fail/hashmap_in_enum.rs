use std::collections::HashMap;
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
enum Bad {
    Variant { data: HashMap<String, u32> },
}

fn main() {}
