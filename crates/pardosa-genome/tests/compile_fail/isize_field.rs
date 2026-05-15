use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    offset: isize,
}

fn main() {}
