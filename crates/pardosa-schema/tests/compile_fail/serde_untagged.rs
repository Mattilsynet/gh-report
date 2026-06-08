use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
#[serde(untagged)]
enum Bad {
    A(u32),
    B(String),
}
fn main() {}
