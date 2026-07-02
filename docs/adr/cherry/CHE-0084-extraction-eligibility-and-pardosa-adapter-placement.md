# CHE-0084. Extraction Eligibility and Pardosa Adapter Placement

Date: 2026-07-02
Last-reviewed: 2026-07-02
Tier: B
Status: Accepted
Crates: gh-report, pardosa

## Related

References: CHE-0029, CHE-0053, CHE-0074, CHE-0055, CHE-0010, PGN-0008, PGN-0010

## Context

gh-report contains mechanisms that could serve other adopters, but "domain-agnostic => library" has only precedents. CHE-0053 extracted storage mechanisms; CHE-0074 kept gh-report on pardosa's public facade; CHE-0055:R7 moved GitHub-shaped pagination and thresholds back into gh-report. The missing decision is the contested boundary plus the legal home for a new extraction that depends on pardosa without reintroducing a cherry-pit -> pardosa edge.

## Decision

Extraction is allowed only when the donor slice can be stated without gh-report org, repository, GitHub API, report, credential, or tenant-policy vocabulary. Those policies stay in gh-report. Domain-agnostic mechanisms become library candidates. Pardosa-dependent new extractions choose option (a): a pardosa-family adapter or add-on crate. The crate depends on `pardosa` through `pardosa::store` or `pardosa::prelude` only, is not a `cherry-pit-*` crate, and is not a backend implementation.

R1 [5]: A gh-report donor slice is extraction-eligible only when its public contract can be named without org, repository, GitHub API, credentials, report rendering, or tenant-policy vocabulary; otherwise it stays in gh-report.

R2 [5]: Domain-agnostic mechanisms are library candidates: filesystem safety, queueing, generic adapter mechanics, diagnostics, or capability carriers may move when their invariant owner and dependency budget are already ruled by an ADR.

R3 [5]: CHE-0055:R7 is the counter-precedent, not an exception: GitHub-shaped pagination and default thresholds moved back to gh-report because their public contract encoded GitHub HTTP semantics; the same test applies to every future extraction.

R4 [5]: A new extracted crate that depends on pardosa chooses placement option (a): a pardosa-family adapter or add-on crate that depends on `pardosa` only through `pardosa::store` or `pardosa::prelude` and reaches no ring-internal module.

R5 [5]: Such a pardosa-family adapter crate is outside the `cherry-pit-*` crate DAG: it MUST NOT be named or wired as a `cherry-pit-*` crate, and it MUST NOT introduce any cherry-pit -> pardosa dependency edge.

R6 [5]: The adapter crate MUST NOT implement or expose pardosa backends, generic backend parameters, or backend traits; PGN-0010 sealing remains untouched, and the crate is an adopter of the public facade, not an extension of the substrate ring.

R7 [5]: The Phase-2 fiber-store example is ruled: extract the generic one-fiber-per-domain-key pardosa mechanism from `gh-report/src/store/mod.rs` to a pardosa-family adapter crate, working name `pardosa-fiber-store`; gh-report retains `DomainEvent` mapping, `key_of` / `org_key_of`, GitHub policy, and the `NativeStore` facade.

R8 [5]: If a candidate cannot pass R1 or would need a `cherry-pit-*` crate with a pardosa dependency, it is not extraction-eligible under this ADR; author a later superseding ADR rather than adding a fourth placement home.

## Consequences

Positive: Phase 2 no longer argues home; the fiber-store moves to a `pardosa-fiber-store`-style crate while preserving CHE-0010 severance and the PGN public facade.

Negative: the pardosa family gains an adopter-facing add-on surface, so Phase 2 must name and version that surface deliberately.

Open / deferred: exact crate name, README, Cargo metadata, and missing CHE-0010 supersession debt remain outside this ADR.

## Rejected Alternatives

**Option (b): standalone third crate outside both DAGs.** Rejected because the dependency and API boundary are already pardosa-shaped. A third governance island would hide that PGN-0008's public facade is the constraining contract.

**Option (c): keep every pardosa-dependent extraction in gh-report.** Rejected for the fiber-store because the generic mechanism has no GitHub vocabulary; gh-report keeps the policy adapter, not the reusable mechanism.

**A `cherry-pit-*` pardosa adapter crate.** Rejected because it would reintroduce a cherry-pit -> pardosa dependency edge. CHE-0029 keeps the cherry-pit DAG acyclic and the standing CHE-0010 severance forbids the coupling.
