use pardosa::store::{EventStore, StoreWriter};
fn coerce<'a>(store: &'a EventStore<u64>) -> StoreWriter<'a, u64> {
    store.reader()
}
fn main() {}
