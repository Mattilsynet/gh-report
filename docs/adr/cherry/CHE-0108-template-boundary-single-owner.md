# CHE-0108. gh-report Template Boundary: One Canonical Owner Per Crossing Fact

Date: 2026-09-01
Last-reviewed: 2026-09-01
Tier: B
Status: Proposed
Crates: gh-report

## Related

References: CHE-0002, CHE-0028, COM-0027, COM-0017, COM-0018, GND-0009, RST-0007

## Context

The owner-detail summary card linked to its drill-down page through a
template-side comparison against a key spelling, plus a hand-written relative
link. Page identity therefore existed twice: once as the emitted page, once as
the template literal. Renaming a control variant severed the branch with every
test green. No ADR owns this crate's server-side template boundary; templates
are hand-authored text the compiler never reads, so the type system stops at
the render call. The corpus governs enforcement-mechanism selection thoroughly
but places no rung for a test that reads template source, leaving the landed
guard unnamed on the ladder.

## Decision

Treat the Rust-to-template crossing as an ownership boundary: every fact that
crosses it has exactly one canonical owner in Rust, the template renders a
derived view of that owner, and the boundary is guarded by a source-text-reading
test in the owning crate's suite.

### What this adds beyond COM-0027

COM-0027 already forbids a second hand-maintained representation of a fact.
It does not say where the boundary of "representation" falls when one side is
not code. This ADR fixes that: the boundary is the render call, the Rust side
is always the canonical side, and the template side is always derived — never
co-equal, never authoritative, never the place a decision is made. That
asymmetry is what makes the rule mechanically checkable, and it is what the
page-identity centralisation implements.

### The derive-versus-test tension

COM-0027:R2 prefers derivation via macros, build scripts, or codegen "so drift
is impossible by construction rather than caught by test". The Rust half of
this pattern complies: the discriminator arrives from an exhaustive match with
no catch-all arm, so an unhandled variant is a compile error. The template half
does not. Codegen of the template was considered and rejected: the templates are
designer-editable presentation artefacts, and generating them would move an
edited-by-humans surface behind a build step, trading a narrow drift risk for a
broad loss of editability. Deriving only the crossing values — rather than the
whole template — is precisely what the typed view-model field already does; the
residual risk is that a future template reintroduces the string comparison, and
that reintroduction is what the test catches. The chosen rung is therefore not a
substitute for derivation but the guard on the one seam derivation cannot reach.

### Enforcement rung, and why stronger rungs were rejected

Per GND-0009:R1/R3/R4 and COM-0017:R4/R5, the rung is named here: a
source-text-reading test in the owning crate's test suite, running
merge-blocking inside the required `build-test-lint` context. Each stronger
rung was rejected for a stated reason.

- Type system / compiler error — unreachable for the template half. The
  templating dialect's text is not seen by the compiler as the Rust owner's
  value set, so no type can constrain it. It is used for the Rust half, which
  is why the discriminator is an exhaustive match.
- Compile-fail test (CHE-0028) — scoped to trybuild cases expressed as Rust
  files. A template is not Rust, so this rung does not admit the check; citing
  it here would be a category error.
- Compiler lint — no lint reads sibling template assets, and authoring one is
  disproportionate to a single crate's presentation seam under the corpus
  complexity budget.
- Shell tripwire in a named merge-gate job — the corpus precedent for
  source-text scanning (COM-0018's projection-lock tripwire and its siblings)
  and therefore the closest competitor. Rejected on evidence: that whitelist
  broke on a routine file move, because a shell tripwire pins paths as text and
  cannot follow a rename. The in-crate test binds to the crate's own manifest
  directory and to the Rust owner's live value set, so the same rename leaves it
  correct. It is a narrower blast radius, run in the same required context, and
  it fails earlier — at test rather than at a separate job.

The test is deliberately not registered as a merge gate in RST-0007's sense: it
has no job identifier of its own and cannot emit the annotation string RST-0007
requires, so declaring it one would create an obligation it cannot discharge. It
is an ordinary test that happens to run in a required context.

R1 [5]: Every fact crossing the Rust-to-template boundary MUST have exactly one canonical owner in Rust, and the template-side occurrence MUST be a view derived from that owner rather than a second hand-maintained representation.

R2 [5]: A template MUST NOT branch on a stringly-typed discriminator; the view model MUST carry the decision already made, as a typed field produced by an exhaustive match with no catch-all arm.

R3 [5]: Page identity MUST have one typed owner in Rust from which both the emitted page and every link targeting it derive, so a rename moves page and link together.

R4 [6]: Reviewers MUST reject any change that adds a template literal restating a spelling Rust already owns, unless the same change makes the template derive that spelling instead.

R5 [5]: Bind this boundary to a source-text-reading test in the owning crate's own suite; it runs merge-blocking inside the required build-test-lint context, and it is the strongest rung feasible while templates remain hand-authored text.

R6 [5]: Such a test MUST assert against the canonical Rust owner's own value set, and MUST fail when it discovers no templates, so it can never pass vacuously.

R7 [6]: A source-text-reading test MUST NOT be declared a named merge gate; it carries no job identifier and emits no annotation string, so RST-0007's citation obligations neither bind nor excuse it.

## Consequences

+ becomes easier: renaming a control variant becomes a compile error in Rust and
a test failure at the template, not a silently dead link; the boundary gains a
named rung reviewers can check.

− becomes harder: a new template branch must first earn a typed field, so small
view tweaks cost a Rust change; the source-text test couples to template syntax.

risks/migration: the drill-down page producer still owns page identity apart
from the link, so R3 is stated ahead of its implementation; the test rung stays
weaker than derivation and should be revisited if build-time template generation
becomes viable.
