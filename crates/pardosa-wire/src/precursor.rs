#[cfg(feature = "blake3")]
#[must_use]
pub fn precursor_hash_of(event_bytes: &[u8]) -> [u8; 32] {
    blake3::hash(event_bytes).into()
}
