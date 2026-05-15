use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
#[serde(default)]
struct Bad {
    x: u32,
    y: u32,
}

fn main() {}
