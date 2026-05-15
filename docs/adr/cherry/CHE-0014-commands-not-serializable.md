# CHE-0014. Commands Not Serializable by Default

Date: 2026-04-24
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0004

## Context

Commands are intent objects dispatched to aggregates. They may stay in-process or cross process boundaries. Events must be serializable (they cross boundaries via persistence and transport). Commands do not share this requirement — many are dispatched in-process and never serialized. Full bounds (`Serialize + DeserializeOwned + Debug + Clone`) make every command pay for serialization whether needed or not. Minimal marker bounds (`Send + Sync + 'static`) let users add derives only when needed.

## Decision

The `Command` trait is a minimal marker: `Send + Sync + 'static`. No
`Serialize`, `Deserialize`, `Debug`, `Clone`, or `PartialEq` bounds.

Users add derives as needed:
- In-process commands: no serde overhead.
- Cross-process commands: add `#[derive(Serialize, Deserialize)]`.
- Debugging: add `#[derive(Debug)]` (most users will do this anyway).

R1 [5]: The Command trait requires only Send + Sync + 'static as
  bounds
R2 [5]: No Serialize, Deserialize, Debug, Clone, or PartialEq bounds
  on the Command trait

## Consequences

- In-process command dispatch avoids serialization overhead entirely.
- No `Debug` requirement means commands can't be logged by the
  framework. Users must derive Debug per-command for diagnostic
  logging.
- No `Clone` requirement means the framework cannot implement
  automatic retry at the Gateway level (the command is moved into
  `handle` and consumed). Callers must reconstruct commands for retry.
- No command audit trail is built into the framework. Only events are
  persisted. Command logging requires user-level serialization.
- All behavior lives in `HandleCommand<C>`, not on the command trait
  itself — clean separation of data (command) from behavior (handler).
