# COM-0031. Signal-to-Noise Ratio in Code, Logs, and Diagnostics

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0010, COM-0019, GND-0005

## Context

COM-0010 makes code obvious; COM-0019 designs for observability.
Both raise *signal*. Neither addresses *noise* — log lines that
never aid diagnosis, abstractions that wrap nothing, comments that
restate code, helpers used once. Noise dilutes signal: a high-volume
log stream hides real anomalies; a layer of one-call indirection
hides the operation. SNR is the design property; this ADR makes it
auditable.

Three options:

1. **Tolerate noise** — common path; signal degrades over time.
2. **Periodic noise sweeps** — episodic; noise re-accumulates.
3. **Per-PR SNR check** — each addition justifies its signal contribution.

Option 3 chosen: cheaper per change, prevents accumulation.

## Decision

Every diagnostic, abstraction, comment, and helper must justify its
signal contribution against the noise it adds. Additions that do
not aid future diagnosis or comprehension are removed.

R1 [5]: Emit log records only at decision points and boundaries
  where the recorded fact would aid post-hoc diagnosis; remove
  log lines that restate the next line of code
R2 [5]: Introduce abstractions only when they hide complexity from
  callers; one-implementation wrappers and pass-through helpers are
  noise and are inlined
R3 [6]: Write comments that record intent the code cannot express;
  comments that paraphrase the code below are deleted in favor of
  clearer code
R4 [5]: Choose log levels so that error and warn correspond to
  actionable conditions; informational chatter belongs at debug or
  trace and is silent by default

## Consequences

- **Pairs with COM-0019.** Observability requires signal density;
  SNR governs how density is achieved without volume inflation.
- **Tension with defensive logging.** Logs added "just in case" fail
  R1; structured event recording at boundaries replaces them.
- **Code-review hook.** Reviewers ask "what does this add?" of
  every new helper, comment, and log line.
