//! ADR-0014 / F3: a downstream type cannot satisfy `Syncable` because
//! the private `sealed::Sealed` supertrait is not nameable outside
//! `pardosa-file`.
use pardosa_file::Syncable;
use std::io::{self, Write};
struct DownstreamSink;
impl Write for DownstreamSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Syncable for DownstreamSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn main() {}
