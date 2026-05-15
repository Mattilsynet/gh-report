use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<u32>,
}

fn main() {}
