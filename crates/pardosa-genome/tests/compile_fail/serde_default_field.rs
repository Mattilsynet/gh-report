use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    #[serde(default)]
    value: u32,
}

fn main() {}
