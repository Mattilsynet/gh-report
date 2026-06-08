use pardosa::store::EventId;
fn _no_external_event_id_mint() {
    let _ = EventId::new(42);
}
fn main() {}
