/// Private supertrait used to strong-seal `EventSafe` per ADR-0014.
///
/// Downstream crates cannot name nor implement [`Sealed`] because the
/// only path to satisfy it is via in-tree `impl sealed::Sealed for ...`
/// or via the workspace `#[derive(GenomeSafe)]` macro (which emits a
/// `Sealed` impl alongside `EventSafe`/`GenomeSafe`). See
/// [ADR-0014](../../../docs/adr/0014-sealed-trait-policy.md) row
/// `EventSafe` and the closure table.
pub trait Sealed {}
