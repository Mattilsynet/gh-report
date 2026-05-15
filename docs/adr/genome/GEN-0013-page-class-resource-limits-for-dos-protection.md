# GEN-0013. Page-Class Resource Limits for DoS Protection

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0001, GND-0001, GND-0005

## Context

Deserialization of untrusted binary data is a denial-of-service vector. A crafted
message with `Vec<T>` containing `count = u32::MAX` in the heap prefix triggers
`Vec::with_capacity(4_294_967_295)` — an instant OOM kill. Deeply nested recursive
types cause stack overflow. These are well-documented attack classes
(serde-rs/serde#744).

pardosa-genome must bound resource consumption before allocation occurs, not after.

## Decision

Introduce a `PageClass` enum defining per-message element budgets and a
`DecodeOptions` struct with configurable resource limits. All limits are enforced
**before** allocation.

**Page classes** (formula: `256 * 16^N`):

| Class | N | Max Elements | Use Case |
|-------|---|-------------|----------|
| Page0 | 0 | 256 | Small config |
| Page1 | 1 | 4,096 | Moderate struct |
| Page2 | 2 | 65,536 | Standard dataset |
| Page3 | 3 | 1,048,576 | Large batch |

**Resource limits** in `DecodeOptions`:

| Limit | Default | Defense |
|-------|---------|---------|
| `max_depth` | 128 | Stack overflow from nested types |
| `max_total_elements` | 256 (Page0) | OOM from crafted Vec/Map count fields |
| `max_uncompressed_size` | 256 MiB | OOM from crafted compressed messages |
| `max_message_size` | 256 MiB | Unbounded processing of bare messages |
| `max_zstd_window_log` | 22 (4 MiB) | Decompressor memory exhaustion |
| `reject_trailing_bytes` | true | Format strictness |

**Enforcement order:**

1. `max_message_size` / `max_uncompressed_size` checked against size headers
   before any buffer allocation.
2. `max_total_elements` checked against Vec/Map count prefix **before**
   `Vec::with_capacity` is called.
3. `max_depth` incremented on entry to struct, enum, option, seq, map, and
   newtype struct deserialization — decremented on exit.

**Element counting:** Each `SeqAccess::next_element` call counts as 1. Each
`MapAccess::next_entry` call counts as 1 (not 2). The budget is global per
message — not per container. Newtype structs are transparent in layout (0 extra
bytes) but increment depth (preventing stack overflow from deep newtype chains).

**Worst-case CPU bound:** `O(max_total_elements * max_depth)`. With Page0
defaults: `256 * 128 = 32,768` operations.

R1 [5]: All resource limits are enforced before allocation occurs
R2 [5]: max_total_elements is checked against Vec and Map count prefix
  before Vec::with_capacity is called
R3 [6]: Element counting is global per message — each SeqAccess and
  MapAccess call counts against a shared budget
R4 [5]: PageClass defines per-message element budgets using the formula
  256 times 16 to the power N

## Consequences

- **Positive:** Prevents OOM from crafted count fields (serde-rs/serde#744).
  Allocation never occurs before limit validation.
- **Positive:** Configurable per use case via `DecodeOptions::for_page_class()`.
  `usize::MAX` disables any individual limit.
- **Positive:** Documented worst-case CPU bound enables capacity planning.
- **Negative:** Default Page0 (256 elements) may be too restrictive for some
  payloads — callers must select appropriate page class.
- **Negative:** Global element counter means nested containers compete for
  budget — a `BTreeMap<String, Vec<u32>>` with 100 entries of 10 elements
  consumes 1,100 of the budget.
