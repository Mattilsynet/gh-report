//! `AdrId` — domain-prefix + zero-padded numeric identifier for an
//! ADR. Newtype over the parsed-from-filename id.
//!
//! Wire shape (load-bearing, frozen for M1.2 onward):
//! 1. `domain: String` — emitted via msgpack's `str` representation.
//! 2. `number: u16` — fixed-width integer per msgpack. `u16` is
//!    sufficient: AFM-0001 reserves `0001..=9999`; `u16::MAX` = 65535
//!    covers that range with margin.
//!
//! Display: `"AFM-0001"`. `FromStr`: parses `"AFM-0001"` → `AdrId`.
//! Construction validates the domain against the prefix set
//! `adr-fmt.toml` declares (mirrored here as a const slice to avoid
//! a runtime config dep on the domain primitive).

use core::fmt;
use core::str::FromStr;

use serde::{Deserialize, Serialize};

/// Domain prefixes mirrored from `adr-fmt.toml` `[domains.*]` entries.
/// Kept as a static slice — runtime config-loading would couple this
/// primitive to `adr_fmt::Config` discovery, which is wrong for a
/// canonical-bytes payload field (the set must be stable across
/// processes that read the same event log).
///
/// Additions here are wire-compatible (new prefixes parse; existing
/// payloads are unaffected). Removals are NOT — they would break
/// replay of events emitted under the removed prefix.
pub const KNOWN_DOMAINS: &[&str] = &[
    "AFM", "CHE", "PAR", "GEN", "SEC", "COM", "GND", "RST", "FLO",
];

/// Parse / validation error for [`AdrId`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdrIdError {
    /// Input did not contain the `-` separator.
    MissingSeparator,
    /// Numeric portion failed to parse as `u16` or was out of range.
    InvalidNumber(String),
    /// Domain prefix is not in [`KNOWN_DOMAINS`].
    UnknownDomain(String),
    /// Numeric portion was not zero-padded to four digits.
    NotZeroPadded(String),
}

impl fmt::Display for AdrIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSeparator => f.write_str("AdrId missing '-' separator"),
            Self::InvalidNumber(s) => write!(f, "AdrId invalid number: {s}"),
            Self::UnknownDomain(s) => write!(f, "AdrId unknown domain: {s}"),
            Self::NotZeroPadded(s) => {
                write!(f, "AdrId number not zero-padded to 4 digits: {s}")
            }
        }
    }
}

impl std::error::Error for AdrIdError {}

/// Newtype over the parsed-from-filename ADR id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AdrId {
    /// Domain prefix, e.g. `"AFM"`, `"CHE"`, `"PAR"`. One of
    /// [`KNOWN_DOMAINS`].
    domain: String,
    /// Zero-padded 4-digit numeric, e.g. `0001..=9999`.
    number: u16,
}

impl AdrId {
    /// Construct an `AdrId` from parts, validating the domain prefix
    /// and the numeric range (`1..=9999` per AFM-0001).
    ///
    /// # Errors
    /// - [`AdrIdError::UnknownDomain`] if `domain` is not in
    ///   [`KNOWN_DOMAINS`].
    /// - [`AdrIdError::InvalidNumber`] if `number == 0` or `> 9999`.
    pub fn new(domain: impl Into<String>, number: u16) -> Result<Self, AdrIdError> {
        let domain = domain.into();
        if !KNOWN_DOMAINS.iter().any(|d| **d == domain) {
            return Err(AdrIdError::UnknownDomain(domain));
        }
        if number == 0 || number > 9999 {
            return Err(AdrIdError::InvalidNumber(number.to_string()));
        }
        Ok(Self { domain, number })
    }

    /// Domain prefix.
    #[must_use]
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Numeric portion.
    #[must_use]
    pub fn number(&self) -> u16 {
        self.number
    }
}

impl fmt::Display for AdrId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:04}", self.domain, self.number)
    }
}

impl FromStr for AdrId {
    type Err = AdrIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (domain, num_str) = s.split_once('-').ok_or(AdrIdError::MissingSeparator)?;
        if num_str.len() != 4 {
            return Err(AdrIdError::NotZeroPadded(num_str.to_string()));
        }
        let number: u16 = num_str
            .parse()
            .map_err(|_| AdrIdError::InvalidNumber(num_str.to_string()))?;
        Self::new(domain, number)
    }
}
