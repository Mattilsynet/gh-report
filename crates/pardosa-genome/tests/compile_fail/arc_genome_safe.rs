// F9d: Arc<T> must NOT implement GenomeSafe (runtime-sharing wrappers do not
// survive serialisation — decode always allocates a fresh Arc; use the inner
// owned type T directly).
use pardosa_genome::GenomeSafe;

fn main() {
    let _ = <std::sync::Arc<String> as GenomeSafe>::SCHEMA_HASH;
}
