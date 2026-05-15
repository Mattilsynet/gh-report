---
name: adr-context
description: Surface the architectural rules applicable to a specific crate via `adr-fmt --context <CRATE>`. Use when an agent (especially oracle, moltke, hopper) needs the binding constraints for code in a crate before deciding or editing, or when the user asks "what rules apply to crate X?"
---

# adr-context

Wrapper around `adr-fmt --context <CRATE>`. Resolves the set of rules
binding on a given crate by walking ADRs in the crate's domain plus
foundation domains (AFM-0015). Output is a flat, numbered rule list
each tagged with its source ADR + Meadows tier.

## Inputs

Crate name as it appears in `adr-fmt.toml` `[corpus.crates]` — e.g.
`adr-fmt`, `cherry-pit-core`, `cherry-pit-gateway`. Not the path; not
the package version.

If the user names a path (`crates/adr-fmt`) or a binary, normalise to
the crate name before invoking.

## Run

```bash
cargo run -p adr-fmt --release -- --context <CRATE>
```

Output structure:

```
# Architecture Rules

These rules are mandatory constraints for all code in crate `<CRATE>`.
Follow every rule without exception.

### <ADR-ID>. <Title>
- <rule text> [<ADR>:R<n>:L<tier>]
- ...
```

Tier suffix `L1..L5` is Meadows leverage (AFM-0011 / AFM-0012). `L1`
is highest leverage (paradigm); `L5` is parameters. When trade-offs
must be made between rules, higher leverage wins.

## Reply shape

When invoked by another agent (oracle, moltke), prefer pointer over
body if the rule list is long:

1. Run the command, capture full stdout.
2. If output > ~40 lines, write to `.ooda/context-<crate>-<ts>.md` and
   return:

   ```
   Crate: <CRATE>
   Rules: <n>  ADRs: <n>  Tiers: L1=<n> L2=<n> L3=<n> L4=<n> L5=<n>
   Artefact: .ooda/context-<crate>-<ts>.md
   Top-leverage (L1–L2):
     - <ADR-ID> <one-line summary>
     - ...
   ```

3. If short, inline the full rule list verbatim.

## Discipline

- Do not paraphrase rules. The trailing tag `[ADR:R:L]` is part of the
  rule and must be preserved when quoting.
- Foundation-domain rules (e.g. `SEC-*`) appear in every crate's
  context — that's AFM-0015 working as intended, not duplication.
- Unknown crate → exit 1 with a list of known crates. Surface verbatim;
  the user typed it wrong.
- Higher-leverage tiers (L1, L2) are commitments; lower (L4, L5) are
  parameters. Flag this distinction when an agent asks "what's most
  important here".

## Typical callers

| Caller | Why |
|--------|-----|
| `oracle` | Architectural input to moltke's Decide phase |
| `moltke` | Constraint set for option enumeration |
| `hopper` | Pre-flight check before editing code in `<crate>` |
| user | "What governs this crate?" |
