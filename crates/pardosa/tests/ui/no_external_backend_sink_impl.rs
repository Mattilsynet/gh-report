use pardosa::store::{AckPosition, BackendError, BackendSink};
struct ForeignSink;
impl BackendSink for ForeignSink {
    fn append(&mut self, _bytes: &[u8]) -> Result<AckPosition, BackendError> {
        unreachable!()
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        unreachable!()
    }
}
fn main() {}
