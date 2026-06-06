# Solon — Story

**Status**: Draft v0.2
**Genre**: Strategic intent. Where to play; how to win.

---

## 1. Solon — the name and the stance

Solon was the Athenian lawgiver who drafted a small set of reforms, swore the citizenry to honour them for a decade, and then left the city so he could not be pressed into amending them. His reforms succeeded not because they were comprehensive but because they were *few*, *observable*, and *ratified*. They were enabling constraints. They made some games illegal so that other games became playable.

This repository takes the same posture. We are not trying to write down every rule of good software design. We are trying to identify the small set of constraints whose enforcement makes correct software easier to build than incorrect software — and to encode those constraints.

The repository is called **Solon** because that is the discipline we are trying to inherit: lay down the foundation, then trust the constraints to do their work.

---

## 2. What we are building

Solon ships a set of Rust library crates and a ADR corpus that, together, let humans and AI agents collaborate on building correct, durable software. The speed comes from the constraints, not in spite of them: when the type system rejects illegal architectures and the event store rejects illegal histories, the search space an agent must explore collapses by orders of magnitude.

The deliverables, end-to-end:

- **`cherry-pit-*`** — the compositional substrate. EDA + DDD +
  hexagonal architecture, expressed as Rust traits and types so that
  illegal compositions (multiple writers per aggregate, async work
  inside the domain, leaky aggregate identity) do not type-check.
- **`pardosa*`** — the behavioural substrate. An append-only event
  store with hash-chained integrity, single-writer-per-stream, a
  canonical wire format (`pardosa-genome`), and a derive macro that
  prevents non-deterministic types from being persisted.
- **`adr-fmt`** (and the forthcoming **`adr-srv`**) — the governance
  plane. The validator that keeps the ADR corpus internally consistent,
  dogfooded against itself; soon the GraphQL service that lets agents
  query "what rules apply to this crate?" without parsing markdown.
- **`gh-report`** — the first real consumer. A GitHub-organisation
  evidence collector that proves the substrate carries non-trivial
  load.

These are libraries first and binaries second. The libraries are the
product; the binaries are evidence that the libraries work.

---

## 3. Where to play — the niche

Solon is opinionated about its addressable problem. The niche is
**high-complexity, durable, intra-organisational workloads that do not
require arbitrary horizontal scale-out**. The boundary is observable,
not adjectival:

| Dimension | In scope | Out of scope |
|-----------|----------|--------------|
| Trust boundary | Intra-org, intra-tenant | Public internet at QPS scale |
| Writer model | Single writer per aggregate | Multi-writer convergence (CRDT, OT) |
| Domain execution | Sync domain, async edges | Distributed transactions across writers |
| Durable state | Sub-PB, single-region | Multi-region active-active |
| Failure model | Crash-fail with recovery | Byzantine, adversarial coordination |
| Identity | Infrastructure-assigned, monotonic | Self-sovereign, content-addressed at scale |
| Consistency | Linearizable per aggregate | Eventual across the whole world |

Concretely: the kind of system Solon expects you to be building is an
org-internal service, an evidence collector, a workflow orchestrator,
a knowledge graph, a governance tool, a durable agent. The kind of
system Solon does **not** help you build is a global content network,
a payment switch, a public messaging substrate, a multi-region database.

The niche is wide enough to cover the majority of software written
inside corporations of any size, and narrow enough that we can be
opinionated. Webscale is a different game with different constraints;
we are not playing it.

---

## 4. How to win — the enabling-constraints thesis

The thesis is that **constraints chosen well are accelerators, not
brakes**. Most software-design advice is additive — add tests, add
review, add documentation, add observability. Solon's bet is
*subtractive*: remove enough degrees of freedom that the remaining
moves are obviously correct.

We work on three planes.

### 4.1 Compositional — cherry-pit

The cherry-pit crate family constrains how you wire a system together.
It makes illegal compositions unrepresentable at the type level:

- **One aggregate per port instance.** You cannot accidentally route
  two aggregates through the same write path.
- **Single writer per aggregate.** Concurrency is two-level: writer
  and reader, never two writers.
- **Sync domain, async infrastructure.** Domain code is pure and
  reasonable; async lives at the edges where it belongs.
- **Pure command handling, infallible apply.** Side effects do not
  hide inside `handle`; state transitions cannot fail mid-flight.
- **Acyclic crate graph; flat public API via re-exports.** The
  architecture is communicated by the file system; an agent can
  navigate it without reading every line.

Anchors in the corpus: CHE-0001 (priority ordering: correctness >
security > energy > response time), CHE-0004 (EDA + DDD + hexagonal),
CHE-0005 (single aggregate per port), CHE-0006 (single writer),
CHE-0008 (pure command handling), CHE-0018 (sync domain, async
infrastructure), CHE-0029 (acyclic crate DAG), CHE-0030 (flat public
API).

### 4.2 Behavioural — pardosa + genome

The pardosa crate family constrains how the system remembers. It
makes illegal histories unobservable:

- **Append-only event log.** History does not retroactively change.
- **Single writer per stream.** Concurrent writers do not exist; the
  question of write conflict does not arise.
- **Hash-chained events with a per-stream frontier.** Tampering with
  history is detectable; the integrity check is local.
- **Canonical wire format (`pardosa-genome`).** Fixed layout, no
  schema evolution within a major version, compile-time rejection of
  non-deterministic types.
- **`GenomeSafe` marker trait, enforced by derive.** Types that cannot
  be deterministically serialised cannot be persisted; the compiler
  prevents the class of bug.

Anchors in the corpus: PAR-0004 (single writer per stream), PAR-0006
(genome as primary serialization), PAR-0008 (publish-then-apply),
PAR-0021 (frontier hash + per-fiber hash chain), GEN-0001 (serde-native
serialization + GenomeSafe), GEN-0002 (no schema evolution), GEN-0004
(reject non-deterministic types), GEN-0006 (zero-copy deserialization
under `forbid(unsafe_code)`).

### 4.3 Doctrinal — the ADR corpus

The ADR corpus is the third plane. Every architectural rule lives in
a document with an id, a parent, a lifecycle, citations, and a
ratification trail. Rules are not floating opinions; they are
typed objects.

This matters because the corpus is **the surface an agent reads to
orient**. `adr-fmt --context <crate>` returns the rules applicable
to a given crate. `adr-fmt --refs <id>` returns who cites a given
rule. `adr-fmt --tree <domain>` returns the rule hierarchy. The
governance plane is queryable, not interpretive.

The corpus organises rules into domains:

| Domain | Prefix | Purpose | Status |
|--------|--------|---------|--------|
| Cherry-pit | `CHE` | Substrate doctrine for the cherry-pit family | Live |
| Pardosa | `PAR` | Event-store and stream doctrine | Live |
| Genome | `GEN` | Canonical-encoding and wire-format doctrine | Live |
| adr-fmt | `AFM` | Validator self-governance | Live |
| Common | `COM` | Cross-cutting design principles (Ousterhout-derived) | Reference |
| Ground | `GND` | Intent + back-briefing (Auftragstaktik-derived) | Reference |
| Flow | `FLO` | Queueing + cost-of-delay (Reinertsen-derived) | Reference |
| Rust | `RST` | Rust-specific toolchain and idiom doctrine | Reference |
| Security | `SEC` | CISQ-aligned security quality | Reference |

Anchors: AFM-0001 (SSOT architecture for ADR governance), AFM-0008
(domain-scoped prefix naming), AFM-0020 (parent-edge tree model),
COM-0017 (mechanized invariant enforcement), COM-0027 (single source
of truth across representations), GND-0009 (mechanized enforcement of
intent).

---

## 5. Research lineage

The corpus did not appear from nowhere. Each domain is the
distillation of a body of work into the smallest set of ratified
rules that operationalises it in Rust:

- **Ousterhout — *A Philosophy of Software Design***. Deep modules,
  pull complexity downward, define errors out of existence, design it
  twice. The COM domain.
- **Vernon — *Implementing Domain-Driven Design***. Aggregates,
  bounded contexts, application services, ACL, event-as-fact. The
  CHE domain (tactical patterns; strategic mapping deferred).
- **Reinertsen — *The Principles of Product Development Flow***.
  Cost of delay, queueing under load, WIP limits, batch size, U-curve
  tradeoff discipline. The FLO domain.
- **Boyd, Moltke (the elder), Bungay — *The Art of Action***.
  Auftragstaktik: directives express intent, not mechanism; deviation
  is permitted and reported; back-briefing precedes action. The GND
  domain — and, not coincidentally, the shape of `AGENTS.md`.
- **Rust idiom and toolchain doctrine**. Pinned stable toolchain,
  workspace-wide `forbid(unsafe_code)`, RPITIT over `async_trait`,
  flat public API. The RST domain.
- **CISQ / OWASP-aligned security quality**. Integrity at trust
  boundaries, bound resource consumption, restrict capabilities by
  default, append-only log for non-repudiation. The SEC domain.

The lineage is not academic. Each rule was admitted to the corpus
because it answered a concrete question we hit while writing
cherry-pit, pardosa, or gh-report.

---

## 6. From research to code

The crate families are the corpus, made executable.

### 6.1 cherry-pit

Eight crates, each one container in the C4-L2 sense, each one a
distinct addressable unit of state or behaviour:

| Crate | Role |
|-------|------|
| `cherry-pit-core` | Type-level invariant carrier. `Aggregate`, `EventStore`, `EventEnvelope`, identity primitives. The substrate ring. |
| `cherry-pit-gateway` | Command ingress; routes commands to single-writer aggregate instances. |
| `cherry-pit-projection` | Read-side: folds events into projections; `ProjectionDriver` orchestrates per-aggregate projections. |
| `cherry-pit-agent` | Long-running aggregate hosts; the `App<...>` composition root. |
| `cherry-pit-web` | Axum-based HTTP surface; binds SEC-0003 resource layers at the library boundary. |
| `cherry-pit-wq` | Work-queue primitives. |
| `cherry-pit-storage` | File-system storage primitives; baseline snapshots subordinate to the event log. |

### 6.2 pardosa

Five crates implementing the append-only behavioural substrate:

| Crate | Role |
|-------|------|
| `pardosa-traits` | Substrate-agnostic trait surface: `EventSafe` (marker for canonically-encodable types), `Validate` + `ValidationCost`, and `Timestamp`. No event-stream or hash-chain types live here. |
| `pardosa-encoding` | Canonical-encoding primitives; `no_std`-clean substrate ring per CHE-0064. |
| `pardosa-derive` | `GenomeSafe` derive macro; compile-time rejection of non-deterministic types per GEN-0004. |
| `pardosa-genome` | Wire format; fixed layout, schema-hash-stamped, frontier-hashed files. |
| `pardosa` | The runtime: streams, fibers, draglines, the actual writer. |

### 6.3 Governance plane

| Crate | Role |
|-------|------|
| `adr-fmt` | Read-only validator and query surface for the ADR corpus. Frozen CLI per AFM-0001. |
| `adr-srv` (Phase 2 v2) | GraphQL service over a `pardosa-genome` projection of the ADR corpus. Read-only in v0.1. |

### 6.4 The consumer

| Crate | Role |
|-------|------|
| `gh-report` | GitHub-organisation evidence collector. First non-trivial consumer of the substrate; load-bearing proof that cherry-pit + pardosa carry real work. |

---

## 7. What we are not solving

Explicit non-goals, recorded here so future contributors (human or
agent) do not have to rediscover them:

- **Webscale.** Multi-region active-active, multi-tenant at internet
  QPS, horizontal scale-out of writers. Different game; different
  constraints; not played here.
- **Frameworks for everything.** Solon is a substrate, not a
  framework. We do not ship application templates, scaffolding tools,
  or "best-practice starter kits". The constraints in the libraries
  are the guidance.
- **Runtime-pluggable architectures.** Trait-object event stores,
  dynamically loaded aggregates, configuration-driven topology.
  Composition is a compile-time concern (CHE-0005:R1).
- **Schema evolution as a first-class feature.** Within a major
  version, the wire format does not change. Cross-major migration is
  a deliberate, ratified operation, not a runtime convenience.
- **Cross-language reads of `pardosa-genome` files.** Rust-only for
  v0.1; cross-language reach is a deferred decision (GEN-0031).
- **Public-facing API stability commitments before v0.3.** We freeze
  invariants; we do not yet freeze surface. See `CLOSURE.md` for the
  versioning ladder.

Each non-goal has a corresponding "yes, but in scope" answer: smaller
scale, smaller blast radius, smaller substrate. That is the trade.

---

## 8. Where we are now

Solon is mid-construction. The cherry-pit family compiles and tests
green; the pardosa family is activated as workspace members; gh-report
runs and produces evidence. The ADR corpus is internally consistent
under `adr-fmt --lint`.

What remains for **v0.1** is enumerated in `docs/CLOSURE.md`. The
short form: the second non-trivial consumer (`adr-srv`, read-only on
pardosa), the persistence migration of `gh-report` onto
`pardosa-genome`, the wire-format hardening of `pardosa-genome` itself
(PAR-0021 frontier hash; F9 event-payload type tightening), the DDD
tactical-pattern alignment of `gh-report`, and an idiomatic-architecture
audit across every workspace crate.

Beyond v0.1 lies **v0.2 (Harden)** — fuzz, proptest, TLA+ for temporal
invariants, Smithy for interface contracts — and **v0.3 (Publish)** —
public-API freeze, semver commitments, docs.rs polish. Neither is
governed by this document.

For the *how we work* layer, see `FOCUS.md` (the standing refinement
recipe) and `docs/c4/roadmap.md` (the live track-level dashboard).
For agent collaboration doctrine, see `AGENTS.md`.

---

## 9. How STORY relates to the ADR corpus

STORY.md is **apex** on questions of *why* and *where to play*. When
STORY and an existing ADR appear to disagree, the disagreement is a
defect in the ADR, not in STORY.

The override is **never silent**. The procedure:

1. A STORY edit that conflicts with one or more existing ADRs must,
   *in the same ratification cycle*, either (a) ship the
   superseding / amended ADR(s) in the same commit-set, or (b) file
   one bd bead per defected ADR with label `story-override`, blocker
   on whatever work would have relied on the defected ADR.
2. Open `story-override` beads are a release blocker. `CLOSURE.md`'s
   v0.1 exit gate fails while any exist.
3. STORY edits and the consequent ADR edits land as **one
   user-ratified commit-set**. There is no "STORY changed, we'll fix
   the ADR later".

The intent of this rule is to make the corpus *converge* under STORY
edits, not to license drift. STORY v1 is drafted so that
`story-override` is empty at landing — every claim above is consistent
with the current corpus. The override mechanism exists for the *next*
strategic shift, when it comes.

ADRs remain binding for **what**. An agent executing a mission reads
the ADRs to know what an invariant is, not STORY. STORY is the
orientation document, read once for context; ADRs are the operating
catalogue, read continuously.

**ADR edits are autonomous-permitted during long-running missions.**
Solon is designed for jobs that run autonomously for long stretches.
ADR drafts, amendments, supersessions, and retirements may land
mid-mission without user-in-the-loop ratification at each edit,
provided every edit is committed to git (the audit trail),
`adr-fmt --lint` stays exit-0 after each commit, a per-ADR audit bead
is filed under `adr-touched,mission:<id>`, and moltke's
mission-complete report enumerates every touched ADR for user review.
The full rules live in FOCUS.md § 6 ("long-autonomous-job exception").
**STORY edits are *not* covered by this exception** — STORY is apex
and its edits remain user-ratified per the override-never-silent rule
above.
