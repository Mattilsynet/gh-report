// F9c: &str must NOT implement GenomeSafe (borrowed types are not genome-safe;
// use owned String / Vec<u8> or bounded EventString<MAX> / EventBytes<MAX>).
use pardosa_genome::GenomeSafe;

fn main() {
    let _ = <&str as GenomeSafe>::SCHEMA_HASH;
}
