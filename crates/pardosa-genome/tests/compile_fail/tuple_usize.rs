use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    pair: (usize, u32),
}

fn main() {}
