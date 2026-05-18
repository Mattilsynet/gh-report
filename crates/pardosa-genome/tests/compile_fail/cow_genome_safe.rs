// F9d: Cow<'_, T> must NOT implement GenomeSafe (borrowed-lifetime hazard —
// generalises the &str / &[u8] exclusion from F9c; use the owned T directly).
use pardosa_genome::GenomeSafe;

fn main() {
    let _ = <std::borrow::Cow<'_, str> as GenomeSafe>::SCHEMA_HASH;
}
