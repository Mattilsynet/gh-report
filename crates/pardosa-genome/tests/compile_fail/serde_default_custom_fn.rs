use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    #[serde(default = "default_value")]
    value: u32,
}

fn default_value() -> u32 {
    42
}

fn main() {}
