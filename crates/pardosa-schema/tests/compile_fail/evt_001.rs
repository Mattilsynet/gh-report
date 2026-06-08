use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
union BadUnion {
    a: u32,
    b: f32,
}
fn main() {}
