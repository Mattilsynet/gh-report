use pardosa_schema::GenomeSafe;

#[derive(GenomeSafe)]
struct Bad {
    marker: (),
}

fn main() {}
