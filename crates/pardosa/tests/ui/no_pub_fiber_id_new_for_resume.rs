#![allow(unreachable_code, unused)]
use pardosa::store::{EventStore, FiberId};
fn forged_fiber_id_still_cannot_resume(store: &mut EventStore<u64>) {
    let forged = FiberId::new(0);
    let mut writer = store.writer();
    let _ = writer.resume_defined(forged, 1u64);
}
fn main() {}
