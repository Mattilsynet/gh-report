use std::collections::HashMap;
use std::collections::BTreeMap;
use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    data: BTreeMap<String, HashMap<String, u32>>,
}

fn main() {}
