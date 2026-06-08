use pardosa_schema::GenomeSafe;
fn main() {
    let _ = <&[u8] as GenomeSafe>::SCHEMA_HASH;
}
