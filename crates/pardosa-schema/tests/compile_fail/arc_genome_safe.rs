use pardosa_schema::GenomeSafe;
fn main() {
    let _ = <std::sync::Arc<String> as GenomeSafe>::SCHEMA_HASH;
}
