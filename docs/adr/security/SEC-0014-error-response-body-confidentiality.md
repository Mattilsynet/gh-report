# SEC-0014. Error Response Bodies — Domain-Lossless, Infrastructure-Opaque

Date: 2026-08-27
Last-reviewed: 2026-08-27
Tier: B
Status: Accepted
Crates: cherry-pit-web, cherry-pit-core

## Related

References: SEC-0007, SEC-0001, CHE-0049, CHE-0015, CHE-0021, COM-0007

## Context

CWE-209 Information Exposure Through an Error Message: a server error rendered verbatim into an HTTP body publishes server-side detail to whoever provoked it. SEC-0007:R1 bans secrets in event payloads, command payloads, and log output — it is silent on response bodies, exactly as SEC-0005:R3 was silent on Origin semantics before SEC-0012. `cherry_pit_web::map_store_error` copies `StoreError`'s `Display` into `ErrorBody.message` for every variant; two of those wrap `Box<dyn Error + Send + Sync>`, an unbounded channel from any storage backend to the wire. Observed payloads carry a filesystem path, an internal routing-index state, and a domain key.

## Decision

Error bodies are lossless for domain-rejected commands and opaque for everything else. SEC-0014 narrows SEC-0007 to the HTTP error-response surface per SEC-0001:R3.

R1 [5]: `DispatchError::Rejected(E)` renders `E`'s `Display` in full — `E` is the consumer's own typed domain error, authored for the caller, and CHE-0049:R4 already ratifies lossless propagation there

R2 [5]: Every other error class — `StoreError`, `BusError`, and all non-`Rejected` `DispatchError` variants — renders a stable machine-readable `code` and a fixed message that is a pure function of that code, with no value interpolated from the error

R3 [5]: The full `Display` of an opaque-rendered error is emitted once via `tracing::error!` at the mapping site, joined to the response by the `X-Correlation-ID` echo CHE-0049:R5 already requires

R4 [6]: The opaque constructor MUST NOT accept a `Display`, a `String`, or any caller-supplied value; message text is reachable only through the code enum, so interpolation is unrepresentable rather than merely forbidden

R5 [5]: `cherry-pit-core` error types keep their lossless `Display`; redaction is a property of the HTTP trust boundary, not of the error, so operators keep full local diagnostics

## Consequences

Clients lose the concurrency-conflict sequence numbers and the store-locked path that previously reached them; the stable `code` plus the correlation id is the supported contract, and operators join to the log line for detail. R4 costs one enum: the wire code and its message live on the same variant, so they cannot drift, and no future mapper can reintroduce interpolation without changing the constructor's type. R5 keeps the redaction boundary in one place — a second implementation inside `cherry-pit-core` would have to be kept in sync with no compiler help.

## Rejected Alternatives

**Redact only the known-bad variant.** `StoreLocked`'s path was the reported leak, but `CorruptData` and `Infrastructure` wrap arbitrary backend errors and already carry a domain key and internal index state. Fixing the named case would leave the larger half open while appearing closed.

**Sanitise `Display` in `cherry-pit-core`.** Rejected per R5: it would blind operator logs to fix an HTTP concern, and every non-HTTP consumer would pay for it.

**Keep interpolation, review it.** Rejected per R4. The leak arrived through a constructor that accepted `impl Display`, and review had already passed over it; removing the parameter removes the defect class.
