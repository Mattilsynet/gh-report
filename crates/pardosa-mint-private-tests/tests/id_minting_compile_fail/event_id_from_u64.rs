use pardosa::store::EventId;
fn _no_external_event_id_from_u64() {
    let _: EventId = 7u64.into();
}
fn main() {}
