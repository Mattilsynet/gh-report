use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
#[serde(tag = "type", content = "data")]
enum Bad {
    A(u32),
    B(String),
}

fn main() {}
