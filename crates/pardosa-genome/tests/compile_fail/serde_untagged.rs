use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
#[serde(untagged)]
enum Bad {
    A(u32),
    B(String),
}

fn main() {}
