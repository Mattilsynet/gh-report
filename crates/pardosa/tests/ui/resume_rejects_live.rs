use pardosa::store::{EventStore, LiveFiber};
fn misuse(store: &mut EventStore<u64>, live: LiveFiber) {
    let mut writer = store.writer();
    let _ = writer.resume(live, 1u64);
}
fn main() {}
