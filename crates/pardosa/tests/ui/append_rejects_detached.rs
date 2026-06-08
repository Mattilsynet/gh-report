use pardosa::store::{DetachedFiber, EventStore};
fn misuse(store: &mut EventStore<u64>, detached: DetachedFiber) {
    let mut writer = store.writer();
    let _ = writer.append(detached, 1u64);
}
fn main() {}
