use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    items: Vec<u32>,
}
fn main() {}
