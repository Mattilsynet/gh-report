use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    offset: Option<isize>,
}
fn main() {}
