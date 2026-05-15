/// M24: `CorrelationContext` does not implement `Default`
/// — callers must use `none()`, `correlated()`, or `new()`
/// (CHE-0039 R2).
use cherry_pit_core::CorrelationContext;

fn main() {
    // This must fail: Default is not implemented.
    let _ctx = CorrelationContext::default();
}
