# AFM-0007. Compiler-Style Diagnostics on Stderr

Date: 2026-04-27
Last-reviewed: 2026-05-01
Tier: B
Status: Superseded by AFM-0014

## Retirement

Superseded-by: AFM-0014
Moved-to-stale: 2026-04-28
Reason: All six output modes now produce unified markdown on stdout.
The compiler-style stderr format was abandoned as adr-fmt expanded
beyond lint-only usage. The print_diagnostic function in report.rs
is dead code. Markdown formatting provides richer presentation
while maintaining greppable rule IDs.
