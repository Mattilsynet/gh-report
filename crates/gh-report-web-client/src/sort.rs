//! Pure comparator and sort-type detection for progressive-enhancement
//! table sorting. Dependency-free (no `web-sys`/`wasm-bindgen`) so it
//! compiles and unit-tests on the host target without `wasm32`.

use std::cmp::Ordering;

/// How to interpret cell text for comparison purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortType {
    /// Compare as floating-point numbers (`%` suffix and `,` thousands
    /// separators are stripped before parsing).
    Numeric,
    /// Compare as ISO-8601 date-prefixed strings (`YYYY-MM-DD...`),
    /// which sort correctly under plain lexicographic ordering.
    Date,
    /// Compare as plain text.
    Text,
}

/// Ascending or descending sort direction, toggled on repeat clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    /// Flip to the opposite direction.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}

/// Parse a `data-sort-type` attribute value into a [`SortType`].
///
/// Returns `None` for a missing or unrecognised attribute, signalling
/// that the caller should fall back to [`detect_sort_type`].
#[must_use]
pub fn parse_sort_type(attr: Option<&str>) -> Option<SortType> {
    match attr {
        Some("numeric") => Some(SortType::Numeric),
        Some("date") => Some(SortType::Date),
        Some("text") => Some(SortType::Text),
        _ => None,
    }
}

/// Auto-detect a column's [`SortType`] by sampling its cell contents.
///
/// Empty/whitespace-only cells are skipped when sampling. A column
/// with no non-empty cells defaults to [`SortType::Text`].
#[must_use]
pub fn detect_sort_type<'a, I: IntoIterator<Item = &'a str>>(cells: I) -> SortType {
    let mut saw_any = false;
    let mut all_numeric = true;
    let mut all_date = true;
    for raw in cells {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        saw_any = true;
        all_numeric &= parse_numeric(trimmed).is_some();
        all_date &= is_iso_date_prefix(trimmed);
    }
    if !saw_any {
        SortType::Text
    } else if all_numeric {
        SortType::Numeric
    } else if all_date {
        SortType::Date
    } else {
        SortType::Text
    }
}

/// Compare two cell strings under the given [`SortType`].
///
/// Numeric comparison treats unparseable values as sorting after all
/// parseable ones (both unparseable falls back to a text compare so
/// the ordering stays a total order).
#[must_use]
pub fn compare_cells(a: &str, b: &str, sort_type: SortType) -> Ordering {
    match sort_type {
        SortType::Numeric => match (parse_numeric(a), parse_numeric(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(b),
        },
        SortType::Date | SortType::Text => a.cmp(b),
    }
}

fn parse_numeric(s: &str) -> Option<f64> {
    let trimmed = s.trim().trim_end_matches('%');
    let cleaned = trimmed.replace(',', "");
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

fn is_iso_date_prefix(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 10
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::{SortDirection, SortType, compare_cells, detect_sort_type, parse_sort_type};
    use std::cmp::Ordering;

    #[test]
    fn parse_sort_type_recognises_numeric() {
        assert_eq!(parse_sort_type(Some("numeric")), Some(SortType::Numeric));
    }

    #[test]
    fn parse_sort_type_recognises_date() {
        assert_eq!(parse_sort_type(Some("date")), Some(SortType::Date));
    }

    #[test]
    fn parse_sort_type_recognises_text() {
        assert_eq!(parse_sort_type(Some("text")), Some(SortType::Text));
    }

    #[test]
    fn parse_sort_type_none_for_missing_attr() {
        assert_eq!(parse_sort_type(None), None);
    }

    #[test]
    fn parse_sort_type_none_for_unrecognised_value() {
        assert_eq!(parse_sort_type(Some("bogus")), None);
    }

    #[test]
    fn sort_direction_toggles_ascending_to_descending() {
        assert_eq!(
            SortDirection::Ascending.toggled(),
            SortDirection::Descending
        );
    }

    #[test]
    fn sort_direction_toggles_descending_to_ascending() {
        assert_eq!(
            SortDirection::Descending.toggled(),
            SortDirection::Ascending
        );
    }

    #[test]
    fn detect_sort_type_all_numeric_is_numeric() {
        let cells = ["1", "42", "3.5"];
        assert_eq!(detect_sort_type(cells), SortType::Numeric);
    }

    #[test]
    fn detect_sort_type_percentages_are_numeric() {
        let cells = ["12%", "100%", "0%"];
        assert_eq!(detect_sort_type(cells), SortType::Numeric);
    }

    #[test]
    fn detect_sort_type_thousands_separator_is_numeric() {
        let cells = ["1,234", "42"];
        assert_eq!(detect_sort_type(cells), SortType::Numeric);
    }

    #[test]
    fn detect_sort_type_all_iso_dates_is_date() {
        let cells = ["2026-07-01", "2026-06-15T10:00:00Z"];
        assert_eq!(detect_sort_type(cells), SortType::Date);
    }

    #[test]
    fn detect_sort_type_mixed_falls_back_to_text() {
        let cells = ["alpha", "42"];
        assert_eq!(detect_sort_type(cells), SortType::Text);
    }

    #[test]
    fn detect_sort_type_all_empty_defaults_to_text() {
        let cells = ["", "   "];
        assert_eq!(detect_sort_type(cells), SortType::Text);
    }

    #[test]
    fn detect_sort_type_skips_blank_cells_when_sampling() {
        let cells = ["", "1", "2", "  "];
        assert_eq!(detect_sort_type(cells), SortType::Numeric);
    }

    #[test]
    fn compare_cells_numeric_orders_by_value_not_lexicographically() {
        assert_eq!(compare_cells("9", "10", SortType::Numeric), Ordering::Less);
    }

    #[test]
    fn compare_cells_numeric_strips_percent_and_commas() {
        assert_eq!(
            compare_cells("1,000%", "999%", SortType::Numeric),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_cells_numeric_unparseable_sorts_after_parseable() {
        assert_eq!(
            compare_cells("N/A", "5", SortType::Numeric),
            Ordering::Greater
        );
        assert_eq!(compare_cells("5", "N/A", SortType::Numeric), Ordering::Less);
    }

    #[test]
    fn compare_cells_numeric_both_unparseable_falls_back_to_text() {
        assert_eq!(
            compare_cells("N/A", "Unknown", SortType::Numeric),
            "N/A".cmp("Unknown")
        );
    }

    #[test]
    fn compare_cells_date_is_lexicographic() {
        assert_eq!(
            compare_cells("2026-01-01", "2026-06-15", SortType::Date),
            Ordering::Less
        );
    }

    #[test]
    fn compare_cells_text_is_lexicographic() {
        assert_eq!(
            compare_cells("alpha", "beta", SortType::Text),
            Ordering::Less
        );
    }

    #[test]
    fn compare_cells_equal_values_are_equal() {
        assert_eq!(
            compare_cells("42", "42", SortType::Numeric),
            Ordering::Equal
        );
        assert_eq!(compare_cells("x", "x", SortType::Text), Ordering::Equal);
    }
}
