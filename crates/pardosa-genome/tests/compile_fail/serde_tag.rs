use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
#[serde(tag = "type")]
enum Bad {
    A { x: u32 },
    B { y: String },
}

fn main() {}
