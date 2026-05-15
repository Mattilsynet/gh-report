---
name: adr-refs
description: List inbound References and Supersedes citations of a target ADR via `adr-fmt --refs <ADR_ID>`. Use when the user asks who references / supersedes / cites an ADR, wants to assess blast radius before changing an ADR, or mentions "adr-fmt --refs".
---

# adr-refs

Wrapper around `adr-fmt --refs <ADR_ID>`. Answers "who points at this
ADR?" — the inverse of reading an ADR's own References block.

## Inputs

The ADR ID is a domain prefix + zero-padded number, e.g. `AFM-0001`,
`SEC-0006`, `CHE-0035`. Case-sensitive. If the user supplies a bare
number or filename, resolve to an ID first (read the file's front
matter or use `adr-fmt --tree <prefix>` to list).

## Run

```bash
cargo run -p adr-fmt --release -- --refs <ADR_ID>
```

Output is one record per inbound citation:

```
- <ADR-ID> [<Relation>] | Tier: <T> | Status: <S> | <Title>
```

Where `<Relation>` is `References` or `Supersedes` (AFM-0009 vocabulary
is exactly three verbs; `Parent` is implicit, not surfaced by --refs).

## Reply shape

```
Target: <ADR-ID> <Title>
Inbound: References=<n>  Supersedes=<n>
Citations:
  - <ADR-ID> [<Rel>] T=<tier> S=<status> <title>
  - ...
```

If the citation list is long (> ~20), summarise by tier:

```
By tier: S=<n> A=<n> B=<n> C=<n> D=<n>
Top (by tier strength):
  - ...
```

## Use cases

| User intent | What to do |
|-------------|-----------|
| "Can I edit AFM-0009 safely?" | Run --refs; surface inbound count + tier mix |
| "Is anyone superseding X?" | Filter relations to `[Supersedes]` |
| "Find dead references" | Compare --refs vs file existence (out of scope for this skill — escalate) |
| "Who references my ADR?" | Use AFM-0024 pattern: --refs to find downstream impact |

## Discipline

- Empty output ≠ error. An ADR with zero inbound citations is a
  valid result; report `Inbound: 0` cleanly.
- Exit 1 means the ID didn't resolve or the corpus walk failed —
  surface stderr verbatim, do not paper over.
- Do not infer relationships not printed. The tool is the authority
  on what's cited; do not synthesize from titles.
