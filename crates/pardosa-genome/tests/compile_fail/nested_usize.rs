use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    counts: Vec<usize>,
}

fn main() {}
