use super::BackendSink;
use super::sealed;
use crate::authoritative::fake::InMemoryBackend;
use crate::durability::AckPosition;
use crate::error::BackendError;
impl sealed::Sealed for InMemoryBackend {}
impl BackendSink for InMemoryBackend {
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError> {
        self.storage.extend_from_slice(bytes);
        let pos = u64::try_from(self.storage.len()).expect("64-bit target enforced at crate root");
        Ok(AckPosition::from_u64(pos))
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        self.synced_to =
            u64::try_from(self.storage.len()).expect("64-bit target enforced at crate root");
        Ok(AckPosition::from_u64(self.synced_to))
    }
}
