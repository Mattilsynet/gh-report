//! `wasm32`-only annotation overlay for `SweepPhase` — deliberately
//! NOT a `sd::Model` node and NOT dressed as one of the five SD
//! component template kinds in [`crate::components`]. adr-fmt-vrycy
//! hotspot (c) rules `SweepPhase` outside the SD grammar entirely: it
//! is a 7-variant discrete control-state enum, not an integral of net
//! flows (no [`crate::sd::Stock`]), not a rate (no
//! [`crate::sd::Flow`]), and not a stateless recompute (no
//! [`crate::sd::Converter`] — a converter cannot remember which phase
//! it was in last tick). [`SweepPhaseBadge`] renders it as a plain
//! control-state label mounted beside the batch subsystem, sharing no
//! `sdt-*` class names with the SD template family so the DOM itself
//! marks the distinction.

use web_sys::Element;

/// A control-state badge showing the current `SweepPhase` label beside
/// the batch subsystem. Not an SD component: no sparkline, no
/// level/inflow/outflow, no `sdt-*` classes — a bare label mount.
pub struct SweepPhaseBadge {
    label_text: Element,
}

impl SweepPhaseBadge {
    #[must_use]
    pub fn mount(container: &Element) -> Option<Self> {
        container.set_inner_html(overlay_skeleton_markup());
        Some(Self {
            label_text: container.query_selector(".phase-overlay-label").ok()??,
        })
    }

    pub fn update(&self, phase_label: &str) {
        self.label_text.set_text_content(Some(phase_label));
    }
}

fn overlay_skeleton_markup() -> &'static str {
    r#"
<span class="phase-overlay-title">SweepPhase (control-state, not SD)</span>
<span class="phase-overlay-label">Init</span>
"#
}
