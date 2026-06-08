//! Mission native-uuid-support-20260526: a #[derive(GenomeSafe)] struct
//! containing a raw uuid::Uuid field must compile under default features
//! (which enable pardosa-schema/uuid).
use pardosa_schema::GenomeSafe;
#[derive(GenomeSafe)]
struct Session {
    id: uuid::Uuid,
    seq: u64,
}
fn main() {
    let _ = <Session as pardosa_schema::GenomeSafe>::SCHEMA_HASH;
}
