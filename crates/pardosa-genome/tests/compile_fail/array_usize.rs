use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    counts: [usize; 4],
}

fn main() {}
