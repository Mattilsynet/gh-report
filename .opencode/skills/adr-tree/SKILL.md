---
name: adr-tree
description: Print the ADR domain tree (parent → child via AFM-0020 parent-edge model) optionally filtered by domain prefix, via `adr-fmt --tree [DOMAIN]`. Use when the user wants to enumerate ADRs in a domain, see the architectural hierarchy, or mentions "adr-fmt --tree".
---

# adr-tree

Wrapper around `adr-fmt --tree [DOMAIN]`. Renders the ADR corpus as
a tree using parent-edge relationships (AFM-0020). Without a filter,
prints all domains; with a domain prefix (e.g. `AFM`, `SEC`, `CHE`),
restricts to that domain's tree.

## Inputs

Domain prefix is **upper-case, three letters**, matching the ADR ID
prefix — e.g. `AFM`, `SEC`, `GND`, `CHE`, `FLO`, `COM`, `PAR`.
Lower-case domain *paths* (`adr-fmt`, `security`) are the directory
names, not the filter argument.

If the user gives a directory name, translate to the prefix:

| Directory | Prefix |
|-----------|--------|
| adr-fmt   | AFM    |
| common    | COM    |
| ground    | GND    |
| genome    | (check `adr-fmt.toml`) |
| pardosa   | PAR    |
| security  | SEC    |
| flow      | FLO    |
| cherry    | CHE    |

When in doubt, run unfiltered first and read the domain headers.

## Run

```bash
# All domains
cargo run -p adr-fmt --release -- --tree

# Single domain
cargo run -p adr-fmt --release -- --tree <PREFIX>
```

Output structure (per domain):

```
## <Domain Name> (<PREFIX>)
  <ADR-ID> <Title> [<Tier>] <Status>
     ├─ <child-ADR-ID> ...
     │  └─ <grandchild> ...
     └─ ...
  (<n> stale)
```

Tier letters: `S` (paradigm) > `A` > `B` > `C` > `D` (parameters), per
AFM-0011. Status is the MADR lifecycle state (`Draft`, `Accepted`,
`Superseded`, `Deprecated`).

The `[also: References X, Supersedes Y]` annotations on a row are
non-tree edges — useful context when assessing impact.

## Reply shape

```
Domain(s): <PREFIX | ALL>
ADRs: <n>  Stale: <n>
Top of tree:
  <verbatim first 5–10 lines, preserving tree art>
Artefact: .ooda/tree-<prefix>-<ts>.md   (if > ~40 lines)
```

For long trees, write the full output to `.ooda/tree-...md` and surface
only the roots + structure summary.

## Use cases

| User intent | Invocation |
|-------------|-----------|
| "List all ADRs about X-domain" | `--tree <PREFIX>` |
| "Show the architecture" | `--tree` (unfiltered) |
| "Find ADR for crate Y" | Map crate → domain via `adr-fmt.toml`, then `--tree <PREFIX>` |
| "Find stale ADRs" | Look at `(N stale)` footer; cross-ref with --refs |

## Discipline

- The tree is parent-edge (AFM-0020). An ADR appears under exactly one
  parent. Cross-domain or supplemental relationships show as `[also: ...]`,
  not as tree branches.
- Do not synthesize a tree if the tool errors. Surface the error.
- Stale count is informational. AFM-0022 governs stub policy; do not
  recommend deleting stale ADRs without reading that ADR first.
- Unknown prefix → empty tree, not an error. Verify the prefix exists
  via the unfiltered output before claiming "no ADRs".
