use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
enum Bad {
    #[serde(default)]
    A { x: u32 },
    B { y: String },
}

fn main() {}
