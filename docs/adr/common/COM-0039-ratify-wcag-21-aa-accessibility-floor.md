# COM-0039. Ratify WCAG 2.1 AA Accessibility Floor

Date: 2026-10-30
Last-reviewed: 2026-10-30
Tier: B
Status: Accepted
Crates: gh-report, gh-report-web-client

## Related

References: COM-0017, COM-0024

## Context

Accessibility shipped first and was never ratified: skip-link, main landmark, sr-only status labels, and the measured contrast floor exist only as templates, CSS, and test assertions. No WCAG or aria term appears anywhere in this corpus, so a future change could silently drop any of these without tripping governance. This ADR turns shipped, test-asserted behaviour into a declared constraint so regressions become actual ADR violations.

## Decision

gh-report declares a WCAG 2.1 AA accessibility floor as a governed constraint: every served page carries a skip-link and main landmark, status colour never stands alone, and all foreground/background pairs clear AA contrast against their composited background. The floor is real and test-asserted at HEAD: the per-page anchor-count manifest is a committed insta golden-file snapshot (`crates/gh-report/src/report/html/tests.rs:522-537`), the measured contrast matrix rests beside `:root` (`crates/gh-report/templates/style.css:66-97`), and the web client asserts `aria-sort` plus Enter/Space keyboard sorting (`crates/gh-report-web-client/src/dom.rs:135-167`). This ADR ratifies that floor and proposes no new tooling.

R1 [5]: Every served page renders the skip-link as its first focusable element, targeting a `<main id="main-content" tabindex="-1">` landmark so keyboard and screen-reader users bypass repeated navigation
R2 [5]: Every colour-coded status indicator backs its colour with an sr-only text label naming the status, so assistive technology never depends on hue alone
R3 [5]: Every foreground and background colour pair meets WCAG 2.1 AA contrast, 4.5:1 for text and 3:1 for non-text, measured against the actual composited background token
R4 [5]: No function or meaning is conveyed by colour alone; every colour-coded status, badge, or sort direction presents the same information in text or aria attributes
R5 [5]: The accessibility floor is enforced by existing committed golden-file snapshot manifests and in-browser assertions, never by documentation convention or code review alone

## Consequences

+ becomes easier: the a11y floor reads as a ratified invariant — reviewers and future changes treat it as load-bearing, and regressions trip the existing golden manifest snapshot and in-browser assertions.
− becomes harder: new colour tokens must clear the measured AA bar, and new served pages must preserve the manifest's anchor-count floor rather than letting the skip-link silently vanish.
risks/migration: documentation-only ratification — no code, template, CSS, or test changes. The snapshot manifest (`dashboard_streaming_manifest`) is the enforcement gate; deleting its anchor assertions alongside a template regression is itself a regression.
