//! Pure comparator and sort-type detection for progressive-enhancement
//! table sorting. Dependency-free (no `web-sys`/`wasm-bindgen`) so it
//! compiles and unit-tests on the host target without `wasm32`.

use std::cmp::Ordering;

/// How to interpret cell text for comparison purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortType {
    /// Compare as floating-point numbers (`%` suffix and `,` thousands
    /// separators are stripped before parsing). Unparseable and `N/A`
    /// values parse as negative infinity, sorting strictly below 0.0
    /// in both ascending and descending directions.
    Numeric,
    /// Compare as ISO-8601 date-prefixed strings (`YYYY-MM-DD...`),
    /// which sort correctly under plain lexicographic ordering.
    Date,
    /// Compare as plain text.
    Text,
    /// Compare typed control states, keeping indeterminate values last.
    Status,
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
        Some("status") => Some(SortType::Status),
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

fn compare_numeric(a: &str, b: &str) -> Ordering {
    match (parse_numeric(a), parse_numeric(b)) {
        (Some(x), Some(y))
            if x.is_infinite()
                && x.is_sign_negative()
                && y.is_infinite()
                && y.is_sign_negative() =>
        {
            a.cmp(b)
        }
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => a.cmp(b),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KnownStatus {
    Fail,
    Partial,
    Pass,
}

fn known_status(value: &str) -> Option<KnownStatus> {
    match value {
        "fail" => Some(KnownStatus::Fail),
        "partial" => Some(KnownStatus::Partial),
        "pass" => Some(KnownStatus::Pass),
        _ => None,
    }
}

fn directed(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

fn compare_status(a: &str, b: &str, direction: SortDirection) -> Ordering {
    match (known_status(a), known_status(b)) {
        (Some(a), Some(b)) => directed(a.cmp(&b), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Compare cell values in a direction.
///
/// - **Status comparisons:** Known states sort by compliance (`pass` > `partial` > `fail`),
///   with indeterminate values (`unknown`, `permission-denied`, `pending`, `N/A`) pinned
///   last in both directions.
/// - **Numeric comparisons:** Unparseable and `N/A` values parse as negative infinity
///   (`f64::NEG_INFINITY`), ensuring they sort strictly below all valid numeric and percentage
///   values (including `0%`) in both directions: ascending places them at the beginning
///   (lowest value), while descending places them at the end (below 0%). Two unparseable/N/A
///   values break ties using deterministic lexicographic ordering.
/// - **Date / Text comparisons:** Standard lexicographic ordering directed by `direction`.
///
/// A status comparison cannot omit its direction. The executable diagnostic-identity
/// harness in `tools/test_web_client_verify.py` verifies the missing-direction error;
/// run it with `python3.12 -B tools/test_web_client_verify.py` from the repository root.
///
/// ```
/// use gh_report_web_client::sort::{SortType, SortDirection, compare_cells_directed};
/// use std::cmp::Ordering;
/// for direction in [SortDirection::Ascending, SortDirection::Descending] {
///     assert_eq!(compare_cells_directed("unknown", "pass", SortType::Status, direction), Ordering::Greater);
/// }
/// ```
#[must_use]
pub fn compare_cells_directed(
    a: &str,
    b: &str,
    sort_type: SortType,
    direction: SortDirection,
) -> Ordering {
    match sort_type {
        SortType::Status => compare_status(a, b, direction),
        SortType::Numeric => directed(compare_numeric(a, b), direction),
        SortType::Date | SortType::Text => directed(a.cmp(b), direction),
    }
}

fn parse_numeric(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Some(f64::NEG_INFINITY);
    }
    let token = trimmed.split_whitespace().next().unwrap_or(trimmed);
    let token = token.trim_end_matches('%');
    if token.eq_ignore_ascii_case("n/a") {
        return Some(f64::NEG_INFINITY);
    }
    let cleaned = token.replace(',', "");
    if cleaned.is_empty() {
        return Some(f64::NEG_INFINITY);
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
    use super::{
        SortDirection, SortType, compare_cells_directed, detect_sort_type, parse_sort_type,
    };
    use std::cmp::Ordering;

    fn compare_cells(a: &str, b: &str, sort_type: SortType) -> Ordering {
        compare_cells_directed(a, b, sort_type, SortDirection::Ascending)
    }

    #[test]
    fn status_sort_type_is_explicit() {
        assert!(parse_sort_type(Some("status")).is_some());
    }

    #[test]
    fn status_sort_keeps_indeterminate_band_stable_in_both_directions() {
        let input = [
            "unknown",
            "pass",
            "permission-denied",
            "partial",
            "",
            "fail",
            "pending",
            "N/A",
        ];
        for (direction, known) in [
            (SortDirection::Ascending, ["fail", "partial", "pass"]),
            (SortDirection::Descending, ["pass", "partial", "fail"]),
        ] {
            let mut rows = input;
            rows.sort_by(|a, b| super::compare_cells_directed(a, b, SortType::Status, direction));
            assert_eq!(&rows[..3], &known);
            assert_eq!(
                &rows[3..],
                &["unknown", "permission-denied", "", "pending", "N/A"]
            );
        }
    }

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
    fn compare_cells_numeric_parses_percentage_with_counts() {
        assert_eq!(
            compare_cells("0% (0/1)", "100% (1/1)", SortType::Numeric),
            Ordering::Less
        );
        assert_eq!(
            compare_cells("2% (1/50)", "10% (1/10)", SortType::Numeric),
            Ordering::Less
        );
        assert_eq!(
            compare_cells("100% (1/1)", "50% (1/2)", SortType::Numeric),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_cells_numeric_unparseable_sorts_before_parseable() {
        assert_eq!(compare_cells("N/A", "5", SortType::Numeric), Ordering::Less);
        assert_eq!(
            compare_cells("5", "N/A", SortType::Numeric),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_cells_numeric_unparseable_sorts_below_zero() {
        assert_eq!(compare_cells("N/A", "0", SortType::Numeric), Ordering::Less);
        assert_eq!(
            compare_cells("N/A", "0%", SortType::Numeric),
            Ordering::Less
        );
    }

    #[test]
    fn compare_cells_numeric_unparseable_sorts_below_negative_values() {
        assert_eq!(
            compare_cells("N/A", "-1", SortType::Numeric),
            Ordering::Less
        );
    }

    #[test]
    fn compare_cells_numeric_both_unparseable_falls_back_to_text() {
        assert_eq!(
            compare_cells("Invalid", "Unknown", SortType::Numeric),
            "Invalid".cmp("Unknown")
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

    #[test]
    fn parse_numeric_maps_na_and_empty_tokens_to_neg_infinity() {
        assert_eq!(super::parse_numeric("N/A"), Some(f64::NEG_INFINITY));
        assert_eq!(super::parse_numeric("n/a"), Some(f64::NEG_INFINITY));
        assert_eq!(super::parse_numeric("N/A (0/1)"), Some(f64::NEG_INFINITY));
        assert_eq!(super::parse_numeric(""), Some(f64::NEG_INFINITY));
        assert_eq!(super::parse_numeric("   "), Some(f64::NEG_INFINITY));
        assert_eq!(super::parse_numeric("%"), Some(f64::NEG_INFINITY));
    }

    #[test]
    fn compare_numeric_infinite_negative_breaks_ties_with_text() {
        assert_eq!(
            compare_cells("N/A (0/1)", "N/A (0/2)", SortType::Numeric),
            Ordering::Less
        );
        assert_eq!(
            compare_cells("N/A (0/2)", "N/A (0/1)", SortType::Numeric),
            Ordering::Greater
        );
    }

    #[test]
    fn compare_cells_numeric_na_sorts_below_zero_and_positive_numbers_both_directions() {
        assert_eq!(
            compare_cells_directed("N/A", "0%", SortType::Numeric, SortDirection::Ascending),
            Ordering::Less
        );
        assert_eq!(
            compare_cells_directed("N/A", "5%", SortType::Numeric, SortDirection::Ascending),
            Ordering::Less
        );
        assert_eq!(
            compare_cells_directed("0%", "N/A", SortType::Numeric, SortDirection::Descending),
            Ordering::Less
        );
        assert_eq!(
            compare_cells_directed("5%", "N/A", SortType::Numeric, SortDirection::Descending),
            Ordering::Less
        );

        let mut rows = ["0%", "N/A", "100%", "50%"];
        rows.sort_by(|a, b| {
            compare_cells_directed(a, b, SortType::Numeric, SortDirection::Ascending)
        });
        assert_eq!(rows, ["N/A", "0%", "50%", "100%"]);

        rows.sort_by(|a, b| {
            compare_cells_directed(a, b, SortType::Numeric, SortDirection::Descending)
        });
        assert_eq!(rows, ["100%", "50%", "0%", "N/A"]);
    }

    #[test]
    fn detect_sort_type_mixed_na_and_percentages_is_numeric_and_sorts_in_both_directions() {
        let cells = ["N/A (0/1)", "2%", "10%", ""];
        let sort_type = detect_sort_type(cells);
        assert_eq!(sort_type, SortType::Numeric);

        let mut rows = ["N/A (0/1)", "2%", "10%", ""];
        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Ascending));
        assert_eq!(rows, ["", "N/A (0/1)", "2%", "10%"]);

        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Descending));
        assert_eq!(rows, ["10%", "2%", "N/A (0/1)", ""]);
    }

    #[test]
    fn detect_sort_type_case_insensitive_na_is_numeric_and_sorts_in_both_directions() {
        let cells = ["n/a", "N/a (0/2)", "5%"];
        let sort_type = detect_sort_type(cells);
        assert_eq!(sort_type, SortType::Numeric);

        let mut rows = ["5%", "n/a", "N/a (0/2)"];
        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Ascending));
        assert_eq!(rows, ["N/a (0/2)", "n/a", "5%"]);

        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Descending));
        assert_eq!(rows, ["5%", "n/a", "N/a (0/2)"]);
    }

    #[test]
    fn detect_sort_type_all_na_samples_is_numeric_and_sorts_in_both_directions() {
        let cells = ["N/A", "n/a", "N/A (0/1)"];
        let sort_type = detect_sort_type(cells);
        assert_eq!(sort_type, SortType::Numeric);

        let mut rows = ["n/a", "N/A (0/1)", "N/A"];
        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Ascending));
        assert_eq!(rows, ["N/A", "N/A (0/1)", "n/a"]);

        rows.sort_by(|a, b| compare_cells_directed(a, b, sort_type, SortDirection::Descending));
        assert_eq!(rows, ["n/a", "N/A (0/1)", "N/A"]);
    }
}
