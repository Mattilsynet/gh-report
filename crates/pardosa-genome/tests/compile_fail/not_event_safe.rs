//! `#[diagnostic::on_unimplemented]` UX gate — exercises the message
//! pointed at users who try to use a non-`EventSafe` type where the
//! sealed trait is required. The blessed path is `#[derive(GenomeSafe)]`
//! (GEN-0036); this fixture verifies the diagnostic carries that
//! pointer rather than a bare "trait bound not satisfied".
use pardosa_traits::EventSafe;

struct NotBlessed;

fn require_event_safe<T: EventSafe>(_: T) {}

fn main() {
    require_event_safe(NotBlessed);
}
