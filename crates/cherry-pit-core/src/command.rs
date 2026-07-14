/// Marker trait for commands.
///
/// Commands represent intent — a request to change state — and may be
/// rejected. They are consumed by value on handling, never borrowed,
/// because intent is one-time. The trait is deliberately minimal: a
/// `Send + Sync + 'static` marker with no `Debug`, `Clone`,
/// `PartialEq`, or serde bounds; consumers add those per-command as
/// needed (CHE-0014:R1–R2). All behavior lives in
/// [`HandleCommand`](crate::HandleCommand).
///
/// Commands entering from remote or retried boundaries should carry an
/// application-level idempotency key in their own payload so handlers
/// can return the original effect (or no new events) after a retry
/// (CHE-0041). [`IdempotencyKey`](crate::IdempotencyKey) is the
/// canonical carrier when the key originates at an HTTP edge.
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
