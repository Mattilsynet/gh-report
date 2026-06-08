use pardosa::store::{EventStore, LiveFiber};
fn use_reader(store: &EventStore<u64>, fiber: LiveFiber) {
    let reader = store.reader();
    let _ = reader.append_to(fiber, 1u64);
}
fn main() {}
