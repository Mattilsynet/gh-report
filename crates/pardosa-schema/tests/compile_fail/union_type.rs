use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
union Bad {
    a: u32,
    b: f32,
}
fn main() {}
