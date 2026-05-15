/// Marker trait for commands.
///
/// Commands represent intent — a request to change state. A command
/// may be rejected. Commands are consumed on handling (moved, not
/// borrowed) because they represent a one-time intent.
/// (CHE-0014: commands not serializable by default.)
///
/// Commands entering from remote or retried boundaries should carry an
/// application-level idempotency key in their own payload. The framework
/// deliberately keeps this trait minimal, but duplicate create/ingress
/// commands must have a stable domain key so handlers can return the
/// original effect or no new events after retry. The
/// [`IdempotencyKey`](crate::IdempotencyKey) newtype is the canonical
/// carrier for that key when it originates at an HTTP edge.
/// (CHE-0041: idempotency strategy.)
///
/// # Design rationale
///
/// - Commands are not required to be serializable by default. Only
///   commands that cross process boundaries (via NATS) need serde
///   derives. In-process commands avoid serialization overhead entirely.
/// - The trait is deliberately minimal — a marker with thread-safety
///   bounds. All behavior lives in [`HandleCommand`](crate::HandleCommand).
/// - No `Debug`, `Clone`, or `PartialEq` — commands carry one-time
///   intent. Consumers may add those bounds per-command as needed.
///   (CHE-0014 R2: no additional supertraits.)
///
/// # Examples
///
/// ```
/// use cherry_pit_core::Command;
///
/// struct PlaceOrder { item: String }
/// impl Command for PlaceOrder {}
/// ```
pub trait Command: Send + Sync + 'static {}
