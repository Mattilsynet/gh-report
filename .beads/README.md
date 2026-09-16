# Beads - AI-Native Issue Tracking

Welcome to Beads! This repository uses **Beads** for issue tracking - a modern, AI-native tool designed to live directly in your codebase alongside your code.

## What is Beads?

Beads is issue tracking that lives in your repo, making it perfect for AI coding agents and developers who want their issues close to their code. No web UI required - everything works through the CLI and integrates seamlessly with git.

**Learn more:** [github.com/steveyegge/beads](https://github.com/steveyegge/beads)

## Quick Start

### Essential Commands (Always Pinned)

Always pin repository discovery using `-C <repo-root>` (or set `export BEADS_DIR=/path/to/gh-report/.beads`). Do not rely on ambient cwd discovery or upward directory walks:

```bash
# Verify store resolution (do not rely on exit 0 alone; check output)
bd -C /path/to/gh-report where
# Output must confirm:
#   /path/to/gh-report/.beads
#     prefix: ghr
#     database: /path/to/gh-report/.beads/embeddeddolt

# Inspect full context metadata
bd -C /path/to/gh-report --readonly context --json

# Create new issues (parented under epic where applicable)
bd -C /path/to/gh-report create "Issue title" --type task --labels "mission:<id>"

# View issues
bd -C /path/to/gh-report list
bd -C /path/to/gh-report show <issue-id>

# Claim an issue
bd -C /path/to/gh-report update <issue-id> --claim

# Close an issue
bd -C /path/to/gh-report close <issue-id> --reason "completed"
```

### Storage and Repository Structure

This repository uses embedded Dolt (database `gh_report`, prefix `ghr`):
- **Tracked scaffold**: Version-controlled in git (`.beads/config.yaml`, `.beads/metadata.json`, `.beads/README.md`, `.beads/.gitignore`, `.beads/hooks/*`).
- **Ignored database & runtime**: Stored locally and ignored by git (`.beads/embeddeddolt/`, `.beads/interactions.jsonl`, backup files, lockfiles).
- **Mutations & git**: Mutations to issues are stored in the Dolt database and do NOT produce git commits. Do not attempt to `git add` database state.
- **Store verification**: An exit code of 0 alone does not verify store identity; defensively verify store resolution by checking that `bd where` outputs prefix `ghr` and database path `<repo-root>/.beads/embeddeddolt`.
- **Diagnostics**: Embedded mode supports `--check=artifacts`, `--check=conventions`, `--check=pollution`, and `--check-health`. Full `bd doctor` without flags requires a Dolt server and returns `embedded_unsupported` here.

## Why Beads?

✨ **AI-Native Design**
- Built specifically for AI-assisted development workflows
- CLI-first interface works seamlessly with AI coding agents
- No context switching to web UIs

🚀 **Developer Focused**
- Issues live in your repo, right next to your code
- Embedded Dolt database with fast local access
- Fast, lightweight, and stays out of your way

🔧 **Repository Integration**
- Tracked configuration and hook shims in repository
- Embedded Dolt issue database isolated from git commits
- Pinned workspace discovery for multi-worktree safety

## Store Verification and Context Checking

An exit code of 0 alone does not prove the expected store answered. Always defensively verify store identity and configuration in scripts and agent sessions:

```bash
# Verify store database, mode, and project ID
set -o pipefail; bd -C /path/to/gh-report --readonly context --json | jq -r '.database, .dolt_mode, .project_id'
```

## Learn More

- **Documentation**: [github.com/steveyegge/beads/docs](https://github.com/steveyegge/beads/tree/main/docs)
- **Quick Start Guide**: Run `bd -C /path/to/gh-report quickstart`
- **Examples**: [github.com/steveyegge/beads/examples](https://github.com/steveyegge/beads/tree/main/examples)

---

*Beads: Issue tracking that moves at the speed of thought* ⚡
