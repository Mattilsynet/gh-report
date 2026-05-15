# COM-0036. Boring Defaults for Obvious-Domain Work

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0009, COM-0027, COM-0026, GND-0002

## Context

COM-0009 makes consistency a complexity reducer; COM-0027 makes one
representation authoritative. Repeatedly solved work still fragments
when every module reinvents the local shape. Once a problem becomes
routine, the architecture should provide one boring path that is
easier to follow than to bypass.

Three options:

1. **Document preferred patterns** — low cost, but copy-paste drift
   persists.
2. **Review every variation** — catches drift late and consumes human
   attention.
3. **Provide defaults** — templates, APIs, and workspace settings make
   the standard path the cheapest path.

Option 3 chosen: obvious-domain work should require recognition, not
fresh design.

## Decision

Once a pattern is solved repeatedly, provide a named workspace default
and route new implementations through it. Variation remains possible,
but must be explicit, owned, and justified.

R1 [5]: Provide a named workspace default for each repeated
  implementation pattern in `Cargo.toml`, `docs/adr/TEMPLATE.md`,
  or a module template
R2 [5]: Route new implementations through the default crate API,
  trait, or template as the canonical construction path
R3 [5]: Capture justified variation as a new ADR or extension point
  with a named owner and review date
R4 [6]: Derive examples, documentation snippets, and generated files
  from the canonical default through build scripts or docs tooling
R5 [5]: Treat repeated copy-paste across modules as a request for a
  default API, macro, or template

## Consequences

- **Reduces local design load.** Developers recognize and apply a
  default instead of inventing a local equivalent.
- **Pairs with subtraction.** COM-0026 still applies; only patterns
  that survive deletion deserve defaults.
- **Can over-standardize.** Defaults may hide useful variation;
  justified variation must remain visible and owned.
- **Increases template responsibility.** Templates and public APIs
  become architectural surfaces, not convenience artifacts.
