/// Verifies that `Command` does NOT imply `Clone` —
/// the trait's supertraits are only `Send + Sync + 'static`
/// (CHE-0014). Callers must add `Clone` explicitly when needed.
use cherry_pit_core::Command;

struct MyCmd;
impl Command for MyCmd {}

fn needs_clone<C: Command>(c: C) {
    // This must fail: Command does not imply Clone.
    let _ = c.clone();
}

fn main() {
    needs_clone(MyCmd);
}
