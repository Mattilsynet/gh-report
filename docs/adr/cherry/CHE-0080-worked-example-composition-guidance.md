# CHE-0080. Worked-Example Composition Guidance

Date: 2026-06-13
Last-reviewed: 2026-06-13
Tier: B
Status: Proposed

## Related

References: CHE-0051, CHE-0005, CHE-0048, CHE-0024, CHE-0017, CHE-0001

## Context

STORY makes type constraints the primary guidance and excludes templates,
scaffolding, and starter kits (`docs/STORY.md:213-219`). Composition knowledge
still spans `App::new`, policy registration, projection tuples, and dispatch
closures (`crates/cherry-pit-app/src/lib.rs:13-29`). The two-aggregate fixture
already separates domain from wiring (`crates/cherry-pit-app/tests/two_aggregate_fixture/mod.rs:10-18`).

## Decision

Ratify worked-example documentation as the supplementary composition guide.
P1 correctness wins over onboarding convenience: examples may explain the
constraints, but generators must not produce hidden architecture.

R1 [5]: Treat library type constraints and compile-time wiring failures as the primary composition guidance

R2 [6]: Provide supplementary agent guidance as a canonical worked-example document grounded in runnable repository fixtures

R3 [5]: Do not ship scaffolding CLIs, template engines, starter projects, or code generators for event-sourced services

R4 [5]: Keep the worked example explanatory; it must not become a normative runtime surface or generated file layout

R5 [6]: Show Aggregate, HandleCommand, EventStore, Projection, Policy, and App wiring in one traceable example path

R6 [5]: Keep consumer wiring explicit, including policy output matching and CorrelationContext threading

R7 [5]: Defer authoring the worked-example document; this ADR ratifies only the boundary and delivery form

## Consequences

+ becomes easier: agents gain a single canonical path to inspect when
  reconstructing composition order, while the libraries remain the executable
  constraint surface.

− becomes harder: users do not get generated project skeletons or a shortcut
  around explicit port, policy, and projection wiring.

risks/migration: no documentation beyond this ADR is authored in this mission.
  A future doc mission should derive the worked example from existing tests and
  keep it warning-clean under the ADR corpus rules.
