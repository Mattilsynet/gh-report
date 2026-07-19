// Shared Mermaid loader for docs/architecture/** pages (mission:arch-docs).
//
// OFFLINE TRADEOFF: this loads Mermaid from a CDN (jsdelivr) at view time.
// Pages in this doc set will NOT render diagrams without network access.
// This is a deliberate tradeoff for phase A: it keeps the doc set free of a
// vendored/bundled copy of Mermaid (multi-hundred-KB) and avoids a build
// step for what is otherwise plain static HTML. If offline viewing becomes
// a hard requirement later, vendor a pinned Mermaid build under
// docs/architecture/assets/vendor/ and swap the import below for a local
// relative path; no other page markup needs to change since every page
// already delegates to this single script.
import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";

mermaid.initialize({
  startOnLoad: true,
  theme: "neutral",
  securityLevel: "loose",
});
