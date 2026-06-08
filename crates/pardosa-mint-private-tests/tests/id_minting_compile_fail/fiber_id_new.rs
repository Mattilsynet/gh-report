use pardosa::FiberId;
fn _no_external_fiber_id_mint() {
    let _ = FiberId::new(42);
}
fn main() {}
