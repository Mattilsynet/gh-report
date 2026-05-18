// F9c: &[u8] must NOT implement GenomeSafe (borrowed types are not genome-safe;
// use owned String / Vec<u8> or bounded EventString<MAX> / EventBytes<MAX>).
use pardosa_genome::GenomeSafe;

fn main() {
    let _ = <&[u8] as GenomeSafe>::SCHEMA_HASH;
}
