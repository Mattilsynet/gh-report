//! `wasm32`-only DOM wiring: finds `table[data-sortable]` elements,
//! makes their headers clickable, and stable-sorts `tbody` rows on
//! click. Progressive enhancement over server-rendered HTML — see
//! CHE-0087.

use leptos::prelude::{Effect, Get, RwSignal, Update};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use web_sys::{Event, HtmlElement, HtmlTableElement, HtmlTableRowElement, HtmlTableSectionElement};

use crate::sort::{SortDirection, SortType, compare_cells, detect_sort_type, parse_sort_type};

#[derive(Clone, Copy)]
struct SortState {
    column: u32,
    direction: SortDirection,
    sort_type: SortType,
}

/// Entry point invoked once at WASM module instantiation.
///
/// Silently does nothing if `document`/`window` are unavailable or no
/// `table[data-sortable]` elements are present — this is progressive
/// enhancement, never a hard requirement for page correctness.
#[wasm_bindgen(start)]
pub fn start() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Ok(tables) = document.query_selector_all("table[data-sortable]") else {
        return;
    };
    for i in 0..tables.length() {
        let Some(node) = tables.item(i) else {
            continue;
        };
        let Ok(table) = node.dyn_into::<HtmlTableElement>() else {
            continue;
        };
        wire_table(&table);
    }
}

fn wire_table(table: &HtmlTableElement) {
    let Some(thead) = table.t_head() else {
        return;
    };
    let Some(header_row) = thead
        .rows()
        .item(0)
        .and_then(|node| node.dyn_into::<HtmlTableRowElement>().ok())
    else {
        return;
    };

    let sort_state: RwSignal<Option<SortState>> = RwSignal::new(None);
    let effect_table = table.clone();
    Effect::new(move |_| {
        if let Some(state) = sort_state.get() {
            apply_sort(&effect_table, &state);
        }
    });

    let header_cells = header_row.cells();
    for i in 0..header_cells.length() {
        let Some(node) = header_cells.item(i) else {
            continue;
        };
        let Ok(th) = node.dyn_into::<HtmlElement>() else {
            continue;
        };
        if th.has_attribute("data-nosort") {
            continue;
        }
        let sort_type = resolve_sort_type(table, &th, i);
        attach_click_handler(&th, i, sort_type, sort_state);
    }
}

fn attach_click_handler(
    th: &HtmlElement,
    column: u32,
    sort_type: SortType,
    sort_state: RwSignal<Option<SortState>>,
) {
    let closure = Closure::<dyn FnMut(Event)>::new(move |_event: Event| {
        sort_state.update(|current| {
            let direction = match current {
                Some(state) if state.column == column => state.direction.toggled(),
                _ => SortDirection::Ascending,
            };
            *current = Some(SortState {
                column,
                direction,
                sort_type,
            });
        });
    });
    let _ignored = th.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref());
    closure.forget();
    let _ignored = th.style().set_property("cursor", "pointer");
}

/// Resolve a column's [`SortType`] from its header's `data-sort-type`
/// attribute, falling back to auto-detection over the current `tbody`
/// contents for that column.
fn resolve_sort_type(table: &HtmlTableElement, th: &HtmlElement, column: u32) -> SortType {
    if let Some(sort_type) = parse_sort_type(th.get_attribute("data-sort-type").as_deref()) {
        return sort_type;
    }
    let Some(tbody) = table
        .t_bodies()
        .item(0)
        .and_then(|node| node.dyn_into::<HtmlTableSectionElement>().ok())
    else {
        return SortType::Text;
    };
    let samples: Vec<String> = collect_rows(&tbody)
        .iter()
        .map(|row| cell_text(row, column))
        .collect();
    detect_sort_type(samples.iter().map(String::as_str))
}

fn apply_sort(table: &HtmlTableElement, state: &SortState) {
    let Some(tbody) = table
        .t_bodies()
        .item(0)
        .and_then(|node| node.dyn_into::<HtmlTableSectionElement>().ok())
    else {
        return;
    };

    let mut texts: Vec<(HtmlTableRowElement, String)> = collect_rows(&tbody)
        .into_iter()
        .map(|row| {
            let text = cell_text(&row, state.column);
            (row, text)
        })
        .collect();
    texts.sort_by(|(_, a), (_, b)| {
        let ordering = compare_cells(a, b, state.sort_type);
        match state.direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        }
    });

    for (row, _) in &texts {
        let _ignored = tbody.append_child(row);
    }
}

fn collect_rows(tbody: &HtmlTableSectionElement) -> Vec<HtmlTableRowElement> {
    let rows = tbody.rows();
    let mut collected = Vec::with_capacity(rows.length() as usize);
    for i in 0..rows.length() {
        if let Some(row) = rows
            .item(i)
            .and_then(|node| node.dyn_into::<HtmlTableRowElement>().ok())
        {
            collected.push(row);
        }
    }
    collected
}

fn cell_text(row: &HtmlTableRowElement, column: u32) -> String {
    row.cells()
        .item(column)
        .map(|cell| cell.text_content().unwrap_or_default())
        .unwrap_or_default()
}
