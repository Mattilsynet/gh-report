use pardosa::store::EventStore;
use std::path::Path;
fn main() {
    let _ = EventStore::<u64>::open_with_migration(Path::new("/tmp/x.pgno"));
}
