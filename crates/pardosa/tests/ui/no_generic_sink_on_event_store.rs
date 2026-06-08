use pardosa::store::EventStore;
fn misuse() -> Option<EventStore<u64, std::io::Cursor<Vec<u8>>>> {
    None
}
fn main() {}
