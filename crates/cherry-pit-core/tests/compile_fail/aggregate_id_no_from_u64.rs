/// M4: `AggregateId` cannot be constructed from `u64` via `From`
/// — only `TryFrom<u64>` is available (CHE-0011 R2).
use cherry_pit_core::AggregateId;

fn main() {
    // This must fail: From<u64> is not implemented.
    let _id: AggregateId = AggregateId::from(5_u64);
}
