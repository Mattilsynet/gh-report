use pardosa_schema::{EventString, GenomeSafe};
#[derive(GenomeSafe)]
struct Good {
    msg: EventString<256>,
}
fn main() {}
