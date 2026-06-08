use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    pair: (usize, u32),
}
fn main() {}
