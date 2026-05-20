# PAR-0027. pardosa-derive Proc-Macro Hygiene Floor

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: PAR-0024, PAR-0025, GEN-0035, GEN-0037

## Context

`pardosa-derive` (PAR-0024) hosts the substrate-agnostic derive
infrastructure; the genome-specific derive moves to
`pardosa-genome-derive` (PAR-0025). Proc-macro output is consumed by
downstream user code that may have any item in scope, including items
that shadow the names the derive emits or refers to. Without an
explicit hygiene floor, expansions break in user code that is itself
correct. The fix is mechanical and well-known — fully-qualified paths,
hygienic locals, no assumptions about call-site imports — but the
discipline has to be written down so future derive additions inherit
it rather than rediscovering it under bug-report pressure.

## Decision

R1 [5]: All trait paths emitted by any `pardosa-derive` or
  `pardosa-genome-derive` macro are fully qualified from the crate
  root (`::pardosa_encoding::Encode`, `::pardosa_traits::EventSafe`,
  `::pardosa_genome::GenomeSafe`). No re-export, no `use` import in
  expansions, no reliance on the call site having any specific item
  in scope.

R2 [5]: All locals introduced by the expansion use hygienic
  identifiers that cannot collide with user code: the
  `__pardosa_<purpose>` prefix is reserved for this purpose.
  Trait-method receivers and parameter names follow the same
  convention.

R3 [5]: Macros may not emit `extern crate` items; the crates they
  depend on are reachable via absolute paths from `::` per R1.

R4 [5]: Span attribution for emitted diagnostics points at the user's
  derive-attribute site, not at the macro's internal expansion. The
  `proc_macro2::Span::call_site()` default is overridden when the
  diagnostic concerns a specific field, variant, or attribute argument.

R5 [6]: A trybuild compile-pass fixture exercising a struct with
  identifiers that would collide under a naive expansion (`fn encode`,
  `let mut buf`, `mod sealed`) is kept under
  `crates/pardosa-derive/tests/compile_pass/`. The same shape is
  carried forward into `pardosa-genome-derive` when PAR-0025 lands.

## Consequences

+ becomes easier: user code that happens to shadow internal derive
  names continues to compile; future derive additions inherit the
  hygiene discipline.

− becomes harder: derive output is more verbose (fully-qualified
  paths). The trade is unconditional correctness against any
  call-site shape.

risks/migration: existing macros are audited against R1–R4 on
  introduction of this ADR; violations are corrected in the audit
  pass before further derives are added.
