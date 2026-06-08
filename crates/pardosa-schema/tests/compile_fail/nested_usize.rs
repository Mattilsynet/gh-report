use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    counts: Vec<usize>,
}
fn main() {}
