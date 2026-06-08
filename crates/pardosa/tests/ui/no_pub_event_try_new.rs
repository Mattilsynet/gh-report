#![allow(unreachable_code, unused)]
use pardosa::store::{Event, EventId, FiberId, Precursor};
fn use_event_try_new_from_adopter_code() {
    let event_id: EventId = unimplemented!();
    let fiber_id: FiberId = unimplemented!();
    let _ = Event::try_new(event_id, fiber_id, false, Precursor::Genesis, [0u8; 32], ());
}
fn main() {}
