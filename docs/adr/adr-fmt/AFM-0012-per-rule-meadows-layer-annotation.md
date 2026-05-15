# AFM-0012. Per-Rule Meadows Layer Annotation

Date: 2026-04-28
Last-reviewed: 2026-05-02
Tier: S
Status: Accepted

## Related

References: AFM-0011, GND-0005, GND-0008

## Context

AFM-0011 established tier classification at the ADR level. Individual
rules within an ADR often target a different leverage layer than the
ADR's overall tier — an S-tier governance ADR might enforce via
layer 5 (structural, B-tier). The old format (`- **R1**: text`)
carried no per-rule metadata, making this invisible.

Options: (1) per-rule layer annotation `R1 [5]: text` — clean,
enables tension analysis; (2) separate metadata table — drift-prone;
(3) inherit ADR tier — lossy. Option 1 chosen: layer is a property
of the intervention, not the decision.

## Decision

Per-rule Meadows layer annotations classify each tagged rule by
the type of systemic intervention it represents, independent of
the ADR's overall tier.

R1 [5]: Write each tagged rule in `RN [L]: text` format where N is
  the sequential rule ID and L is the Meadows leverage layer (1-12)
R2 [5]: Select rule ID, layer, and text in a single pass using
  the parser regex `^R(\d+)\s*\[(\d+)\]:\s*(.+)` — apply to every
  tagged rule in every `## Decision` section
R3 [5]: Apply T016 to validate layer range 1–12 for all statuses
  and permit Draft and Proposed ADRs to carry tagged rules without
  exemption from the format requirement
R4 [7]: Hold tension between an ADR's tier and its rules'
  Meadows layers as a derivable property of the per-rule annotation;
  a T019 violation fires iff `layer_to_tier(rule.layer).rank() <
  adr_tier.rank()` (rule operates at higher leverage than the ADR
  tier warrants — asymmetric bound); equal or lower leverage passes
  silently; the layer annotation is the load-bearing input for any
  future tier-tension diagnostic in `--lint`
R5 [6]: Render rules with layer suffix in `--context` output using
  the global identifier format `[PREFIX-NNNN:RN:LN]` — apply to
  every rule extracted by `adr-fmt --context`

## Consequences

- **Tension visibility.** S-tier ADRs with layer-5 rules carry
  tension derivable as `|0–2|` — governance enforced structurally.
  Under the asymmetric rule (R4), such rules pass because the rule
  layer rank (2) is not less than the ADR tier rank (0). Expected,
  not defect. As of AFM-0021 no viewer mode renders this; the
  property is derivable from the persisted annotation for any future
  lint-stage diagnostic.
- **No dual-format.** Old `- **R1**: text` no longer parses. All 12
  files migrated atomically.
- **Layer ≠ tier.** Layer = intervention type; tier = significance.
  Orthogonal classifications providing richer architectural insight.
- **R0 removed.** ADRs without tagged rules produce empty vec + T016
  warning.
