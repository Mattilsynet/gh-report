use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    counts: [usize; 4],
}
fn main() {}
