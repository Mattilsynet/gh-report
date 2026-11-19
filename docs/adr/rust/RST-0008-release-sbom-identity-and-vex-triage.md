# RST-0008. Release SBOM Identity and Repository-Held VEX Triage

Date: 2026-11-19
Last-reviewed: 2026-11-19
Tier: B
Status: Accepted

## Related

References: RST-0004, SEC-0009, SEC-0013

## Context

RST-0004 and SEC-0009 audit the workspace dependency graph and fail the build
on advisories. Neither states what is published to Dependency-Track, how the
published inventory is identified, or where an accepted-risk decision lives
after the audit gate passes. The release pipeline therefore uploaded a
source-tree scan under a per-tag project version that started empty on every
release, and the two compile-time advisory ignores in `deny.toml` had no
counterpart in the delivered project. This ADR fixes the identity, the
inventory scope and the triage home.

## Decision

The delivered inventory is the shipped artefact: one constant project identity,
an image-digest root, a compile-time-only-free cargo closure delivered by the
deploying job; triage lives in a reviewed repository VEX file.

R1 [5]: The Dependency-Track identity is the literal pair
  `projectName=gh-report`, `projectVersion=prod`; no workflow derives
  either from a tag, commit SHA, dispatch input or branch name
R2 [5]: The uploaded BOM is one CycloneDX 1.5 document whose metadata
  component is the pushed image as a `pkg:oci` purl carrying its
  `sha256:<hex>` digest; its components are the release cargo inventory
  merged with a scan of that digest
R3 [5]: The released cargo inventory is the normal-edge closure of
  `gh-report` for the release platform, pruned at every package with a
  proc-macro target; compile-time-only crates are excluded and named in the
  step log, never hand-edited out
R4 [6]: The BOM is generated in the job that pushes the image, from the
  pushed digest, and delivered by the job that deploys it, from the same
  workflow artifact after Cloud Run readiness
R5 [6]: Triage decisions live in `crates/gh-report/dtrack-vex.json` and are
  applied only by a `workflow_dispatch` job gated to `main`, a protected
  environment, and closed literal project inputs
R6 [5]: An advisory excluded by R3 stays recorded in `deny.toml` under
  SEC-0013 and MUST NOT be seeded into the VEX file until a shipped
  component carries it

### Excluded advisories

`RUSTSEC-2026-0374` (`basic-toml` 0.1.10, reached through `askama_derive`'s
proc-macro) and `RUSTSEC-2024-0436` (`paste` 1.0.15, reached through the wasm
client's Leptos proc-macros) are compile-time-only and absent from the released
inventory. `basic-toml` is reachable only behind the proc-macro prune point;
`paste` is not in the server graph at all (`cargo tree -p gh-report --target
x86_64-unknown-linux-gnu -i paste` reports no match) and is a proc-macro in the
wasm-client graph, so it is not in the committed wasm bundle either. Their
acceptance remains SEC-0013's, recorded in `deny.toml`; absence from the BOM is
not evidence that either left the workspace dependency graph.

## Consequences

+ becomes easier: triage survives releases because the project version is
  constant; the inventory root names the image that runs; the exclusion rule
  is reviewable in the step log and enforced by the merge tool's `--check`
− becomes harder: a release-configuration change that ships a new artefact
  class (assets, embedded wasm) is invisible until the closure rule is
  extended; every release now needs registry access for the image scan
risks/migration: existing tag-versioned projects in Dependency-Track are left
  untouched and untriaged; `distroless/cc-debian12` has no package database,
  so OS-package yield is approximately zero and the gain is OCI identity, not
  OS inventory; the committed wasm client's runtime crate graph is embedded as
  bytes and is not inventoried (ghr-f82ow); the upload is accepted for
  asynchronous processing, so a green job is not completed analysis; the
  release step fails closed below a 200-crate shipped closure, so an inventory
  that legitimately shrinks past that floor needs a deliberate re-decision
  rather than a silent pass.
