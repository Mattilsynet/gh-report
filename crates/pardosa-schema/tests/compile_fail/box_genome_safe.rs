use pardosa_schema::GenomeSafe;

fn main() {
    let _ = <Box<u32> as GenomeSafe>::SCHEMA_HASH;
}
