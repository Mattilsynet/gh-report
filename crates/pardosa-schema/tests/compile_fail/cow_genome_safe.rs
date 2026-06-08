use pardosa_schema::GenomeSafe;
fn main() {
    let _ = <std::borrow::Cow<'_, str> as GenomeSafe>::SCHEMA_HASH;
}
