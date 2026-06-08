use pardosa::store::EventStore;
fn use_reader(store: &EventStore<u64>) {
    let reader = store.reader();
    let _ = reader.sync();
}
fn main() {}
