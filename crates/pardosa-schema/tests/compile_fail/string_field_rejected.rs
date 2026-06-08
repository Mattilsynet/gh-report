use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Bad {
    name: String,
}
fn main() {}
