# GEN-0037. Derive Diagnostic Catalog and Encode/Decode Emission

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0029, GEN-0035, GEN-0036, PAR-0024

## Context

GEN-0035 freezes the canonical wire format. GEN-0036 introduces the
sealed `EventSafe ← GenomeSafe ← GenomeOrd` stack and defers `Encode`
emission (F2) to A2.2 so the supertrait strengthening lands atomically
with derived bodies. The `GenomeSafe` derive already rejects
non-deterministic and layout-incompatible inputs (HashMap, usize,
`#[serde(flatten)]`), but error messages are free-text — downstream
tooling, IDE diagnostics, and trybuild fixtures match on prose, which
drifts when wording changes. The catalog also lacks coverage for raw
pointers and function pointers, which compile silently through the
existing arms despite having no canonical wire form. Sub-mission A2.2a
introduces structured diagnostic codes and lands the struct/enum
`Encode`/`Decode` emission required for A2.2b to strengthen
`EventSafe: Encode + Sealed`.

## Decision

Adopt a stable `EVT-NNN` diagnostic code catalog for `GenomeSafe`
derive rejections and emit canonical `Encode`/`Decode` impls
alongside the existing `Sealed`/`EventSafe`/`GenomeSafe` impls.

### EVT diagnostic catalog (N = 13)

Each code maps to one canonical compile-fail fixture at
`crates/pardosa-genome/tests/compile_fail/evt_NNN.rs`. The catalog is
growth-friendly: new codes append; existing codes are stable.

| Code    | Rejection                                              |
| ------- | ------------------------------------------------------ |
| EVT-001 | union (unsupported data shape)                         |
| EVT-002 | `HashMap` field (non-deterministic iteration)          |
| EVT-003 | `HashSet` field (non-deterministic iteration)          |
| EVT-004 | `usize` field (platform-dependent size)                |
| EVT-005 | `isize` field (platform-dependent size)                |
| EVT-006 | `#[serde(flatten)]` (breaks fixed layout)              |
| EVT-007 | `#[serde(untagged)]` (bypasses variant tagging)        |
| EVT-008 | `#[serde(default)]` (inert; GEN-0029)                  |
| EVT-009 | `#[serde(tag = "…")]` (internally tagged enum)         |
| EVT-010 | `#[serde(content = "…")]` (adjacently tagged enum)     |
| EVT-011 | `#[serde(skip_serializing_if = "…")]` (cond. omission) |
| EVT-012 | raw pointer field (`*const T` / `*mut T`)              |
| EVT-013 | function pointer field (`fn(..) -> ..`)                |

Legacy compile_fail fixtures (`hashmap_field.rs`, etc.) are retained
as additional regression coverage; canonical catalog fixtures live at
`evt_NNN.{rs,stderr}`. Rejection messages are formatted
`"EVT-NNN: GenomeSafe: <reason>"`. Tooling matches on the `EVT-NNN`
prefix; the prose after the colon is advisory and may evolve.

### Encode/Decode emission

The derive emits two additional impls per input:

- **Struct.** Named, tuple, and unit fields encode/decode back-to-back
  in declaration order per GEN-0035:R3.
- **Enum.** One `u8` discriminant byte (the explicit `repr(u8)` literal
  per GEN-0035:R4) followed by variant payload in declaration order.
  Decode dispatches on the byte; unknown bytes return
  `DecodeError::InvalidDiscriminant`.

The codec where-clause adds `T: ::pardosa_encoding::Encode +
::pardosa_encoding::Decode` for every type parameter. Parameters
identified in `BTreeMap` key position additionally receive `T: Ord`
to satisfy upstream `BTreeMap<K, V>: Decode where K: Ord` bounds.

`Encode` and `Decode` are re-exported from `pardosa-genome` (mirroring
the `EventSafe` re-export pattern in GEN-0036) so derive-emitted paths
resolve in downstream user code without a direct `pardosa-encoding`
dependency.

### Well-formedness prerequisites

Beyond the curated catalog, the derive enforces emission prerequisites
as un-coded `syn::Error` diagnostics. These are not EVT entries because
they prevent emission entirely rather than reject a user-reachable
pattern worth curating:

- **Enum `#[repr(u8)]` with explicit literal discriminants on every
  variant** (GEN-0035:R4). Required so the discriminant byte is
  resolvable at derive time and stable under variant reordering.

Promotable to EVT-014 in a future sub-mission if usage frequency
justifies catalog framing.

### Rules

R1 [4]: Every derive rejection that targets a user-reachable input
  pattern is tagged with a stable EVT-NNN code as the first colon-
  delimited segment of the error message
R2 [4]: Each EVT code has at least one canonical compile-fail fixture
  at crates/pardosa-genome/tests/compile_fail/evt_NNN.rs with a frozen
  stderr snapshot
R3 [4]: EVT codes are append-only — existing codes are not renumbered
  even when rejections are removed; new rejections take the next
  unused integer
R4 [4]: The derive emits Encode and Decode impls alongside the Sealed,
  EventSafe, and GenomeSafe impls — never as a separate macro and
  never optional
R5 [4]: Enum encoding uses the explicit repr(u8) discriminant literal
  per GEN-0035:R4 — the derive rejects enums missing #[repr(u8)] or
  with non-literal variant discriminants as a well-formedness
  prerequisite, not an EVT catalog entry
R6 [4]: Well-formedness prerequisites are un-coded syn::Error
  diagnostics and may be promoted to EVT codes if usage frequency
  justifies catalog framing
R7 [4]: Encode and Decode are re-exported from pardosa-genome so
  derive-emitted paths resolve in downstream user code without a
  direct pardosa-encoding dependency

## Consequences

- **Positive:** structured EVT-NNN codes give downstream tooling a
  stable match target decoupled from message prose.
- **Positive:** the canonical fixture set documents the rejection
  surface in one place — adding a rejection is mechanically gated on
  adding an `evt_NNN.{rs,stderr}` pair.
- **Positive:** struct and enum Encode/Decode emission completes the
  derive's canonical-bytes contract per GEN-0035, unblocking A2.2b's
  `EventSafe: Encode + Sealed` strengthening.
- **Positive:** raw-pointer and function-pointer rejections close
  silent encoding gaps that would otherwise produce non-portable wire
  output if wrapped in `Vec` or `Option`.
- **Negative:** enums now require `#[repr(u8)]` and explicit
  discriminants; future users face steeper onboarding.
- **Negative:** the EVT-NNN catalog is a published surface — removals
  must be additive (integer stays reserved) to avoid breaking matchers.
- **Negative:** message prose after the colon remains advisory; tools
  matching on prose drift when wording changes.
