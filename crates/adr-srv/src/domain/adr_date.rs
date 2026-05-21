//! `AdrDate` — calendar-date (no time component) representation for
//! the `date` and `last_reviewed` fields of an ADR.
//!
//! Why a bespoke newtype rather than `jiff::civil::Date`?
//!
//! `jiff::civil::Date` does not implement `serde::Serialize`/`Deserialize`
//! by default (jiff's serde feature gates a string representation via
//! `jiff::fmt::serde`, not the value itself). Civil-date semantics
//! preserved via a wire-shape-explicit `(year, month, day)` newtype:
//! 4 bytes total via msgpack, no jiff foreign-impl dependency.
//!
//! `jiff::civil::Date` is still the parsed-from-ADR-frontmatter
//! representation at the M1.3 boundary; M1.3 will convert at the
//! event-construction call site.

use core::fmt;

use serde::{Deserialize, Serialize};

/// Calendar date with no time component. Wire shape:
/// `(i16 year, u8 month, u8 day)` via msgpack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AdrDate {
    /// Proleptic Gregorian year. `i16` admits negative years (BCE)
    /// for symmetry with `jiff::civil::Date::MIN.year() = -9999`;
    /// ADR dates in practice are 2020+.
    year: i16,
    /// 1..=12.
    month: u8,
    /// 1..=31 (validated against month at construction).
    day: u8,
}

/// Construction error for [`AdrDate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdrDateError {
    /// `month` was 0 or > 12.
    InvalidMonth(u8),
    /// `day` was 0 or exceeded the days in `month`.
    InvalidDay { month: u8, day: u8 },
}

impl fmt::Display for AdrDateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMonth(m) => write!(f, "AdrDate invalid month: {m}"),
            Self::InvalidDay { month, day } => {
                write!(f, "AdrDate invalid day {day} for month {month}")
            }
        }
    }
}

impl std::error::Error for AdrDateError {}

impl AdrDate {
    /// Construct from year/month/day with validation.
    ///
    /// # Errors
    /// - [`AdrDateError::InvalidMonth`] if `month` is 0 or > 12.
    /// - [`AdrDateError::InvalidDay`] if `day` is 0 or exceeds
    ///   the maximum for `month` (leap-year-aware for February).
    pub fn new(year: i16, month: u8, day: u8) -> Result<Self, AdrDateError> {
        if month == 0 || month > 12 {
            return Err(AdrDateError::InvalidMonth(month));
        }
        let max_day = days_in_month(year, month);
        if day == 0 || day > max_day {
            return Err(AdrDateError::InvalidDay { month, day });
        }
        Ok(Self { year, month, day })
    }

    /// Year component.
    #[must_use]
    pub fn year(self) -> i16 {
        self.year
    }
    /// Month component (1..=12).
    #[must_use]
    pub fn month(self) -> u8 {
        self.month
    }
    /// Day component (1..=days-in-month).
    #[must_use]
    pub fn day(self) -> u8 {
        self.day
    }
}

impl fmt::Display for AdrDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

fn days_in_month(year: i16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_leap_year(year: i16) -> bool {
    let y = i32::from(year);
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
