// Pardosa relies on usize == u64 for index arithmetic. Reject 32-bit targets
// at compile time to prevent silent truncation in `fiber.current().value() as usize`.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("pardosa requires a 64-bit target (usize must be at least 8 bytes)");

pub mod dot;
pub mod dragline;
pub mod error;
pub mod event;
pub mod fiber;
pub mod fiber_state;

pub use dragline::{AppendResult, Dragline};
pub use error::PardosaError;
pub use event::{DomainId, Event, Index};
pub use fiber::Fiber;
pub use fiber_state::{FiberAction, FiberState, LockedRescuePolicy, MigrationPolicy};
