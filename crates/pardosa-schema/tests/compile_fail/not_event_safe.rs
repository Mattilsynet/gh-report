use pardosa_wire::EventSafe;
struct NotBlessed;
fn require_event_safe<T: EventSafe>(_: T) {}
fn main() {
    require_event_safe(NotBlessed);
}
