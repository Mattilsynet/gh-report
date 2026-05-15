use pardosa_genome::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    offset: Option<isize>,
}

fn main() {}
