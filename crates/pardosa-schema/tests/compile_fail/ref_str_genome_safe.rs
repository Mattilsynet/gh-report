use pardosa_schema::GenomeSafe;
fn main() {
    let _ = <&str as GenomeSafe>::SCHEMA_HASH;
}
