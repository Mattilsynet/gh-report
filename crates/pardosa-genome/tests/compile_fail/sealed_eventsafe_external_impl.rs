// Falsifier: external crates must not be able to impl EventSafe for their own
// types. The sealing chain `EventSafe: sealed::Sealed` prevents this because
// `sealed::Sealed` lives in a module whose only impls are inside trusted
// pardosa-* crates.

use pardosa_genome::EventSafe;

struct Outsider;

impl EventSafe for Outsider {}

fn main() {}
