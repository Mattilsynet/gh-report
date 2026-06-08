use pardosa_schema::EventSafe;
use pardosa_wire::Encode;
struct Outsider;
impl Encode for Outsider {
    fn encode(&self, _out: &mut Vec<u8>) {}
}
impl EventSafe for Outsider {}
fn main() {}
