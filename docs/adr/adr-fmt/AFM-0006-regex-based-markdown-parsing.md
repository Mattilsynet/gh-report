# AFM-0006. Regex-Based Markdown Parsing Over AST Parsing

Date: 2026-04-27
Last-reviewed: 2026-04-29
Tier: D
Status: Accepted

## Related

References: AFM-0004

## Context

`adr-fmt` must extract structural information from markdown:
titles, metadata, section headings, relationships, prose. ADR
files use a constrained subset — ATX headings, fenced code blocks,
`Key: Value` metadata. A full AST parser adds dependency weight
without unlocking needed capabilities. Lexical token validation
(e.g. ADR ID shape) is a separate concern from markdown structure.

## Decision

Parse ADR markdown using line-by-line iteration with compiled regex
patterns. Do not depend on any markdown AST parser.

R1 [9]: Compiled regex patterns drive markdown structural
  extraction (headings, sections, fences, relationships) under an
  enum state machine; lexical validators for fixed-shape tokens
  (ADR IDs, layer numbers) may use byte-level checks
R2 [9]: Word counting accumulates per-section, excluding lines
  inside fenced code blocks by tracking fence open/close state
R3 [9]: Only ATX headings and fenced code blocks are supported;
  setext headings and indented code blocks are not recognized
R4 [12]: Reassess if ADRs require tables or structure complex
  enough that line-oriented parsing produces ambiguous results

## Consequences

The `regex` crate is the only structural parsing dependency. Every
markdown extraction rule is a visible regex pattern. Fixed-shape
token validators are byte-level for clarity. ADR authors must
follow the constrained markdown subset; unrecognized headings
produce missing-section warnings. New structural elements need one
regex plus one extraction branch.
