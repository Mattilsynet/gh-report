//! Host-facing error surface for the `domain-app` plugin boundary.

/// Failure surface for host operations against the `domain-app` guest
/// component (SEC-0013, CHE-0105/0106/0107).
///
/// `#[non_exhaustive]` (constraint 11, PGN-0006:R1/R2): new variants are
/// non-breaking additions, never removed.
///
/// A trap inside `apply-event` gets its own `ApplyEventTrapped` variant
/// (G-E) and is never swallowed or mapped onto a domain error: CHE-0009:R1/
/// R2 make `apply-event` total and infallible, so a trap there is a bug
/// signal, not a condition, and must stay one all the way to the caller.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HostError {
    /// The component bytes failed to parse or validate.
    #[error("failed to load domain-app component: {0}")]
    ComponentLoad(#[source] wasmtime::Error),

    /// Linking or instantiating the component into a fresh store failed.
    #[error("failed to instantiate domain-app component: {0}")]
    Instantiate(#[source] wasmtime::Error),

    /// The call exhausted its compile-time fuel budget
    /// ([`CALL_FUEL_LIMIT`](crate::CALL_FUEL_LIMIT)) before completing.
    #[error("guest call exhausted its fuel budget")]
    FuelExhausted,

    /// The call exceeded its configured memory or store-size limit.
    #[error("guest call exceeded its memory or store-size limit")]
    MemoryExhausted,

    /// `apply-event` trapped inside the guest. CHE-0009:R1/R2 make
    /// `apply-event` total and infallible over well-formed input, so a
    /// trap here is a bug signal and is deliberately kept distinct from
    /// every other failure variant — never mapped onto a domain error.
    #[error("apply-event trapped inside the guest: {0}")]
    ApplyEventTrapped(#[source] wasmtime::Error),

    /// `handle-command` trapped inside the guest (a call-shape violation
    /// or guest bug distinct from a returned domain `HandleError`).
    #[error("handle-command trapped inside the guest: {0}")]
    HandleCommandTrapped(#[source] wasmtime::Error),

    /// Guest output failed membrane validation (constraint 7,
    /// SEC-0013:R4, SEC-0002:R1/R3/R4): guest output is untrusted input
    /// and must be validated at the boundary before the host trusts it.
    #[error("guest output failed membrane validation: {0}")]
    InvalidGuestOutput(String),
}
