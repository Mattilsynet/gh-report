use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    offset: isize,
}
fn main() {}
