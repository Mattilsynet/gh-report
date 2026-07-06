//! Dashboard coverage tier configuration.
//!
//! Thresholds are set via CLI arguments with sensible defaults.

use crate::config::org::OrgHelpConfig;
use crate::error::ConfigError;

/// Dashboard rendering configuration.
#[derive(Debug, Clone, Default)]
pub struct DashboardConfig {
    /// Coverage tier thresholds.
    pub tiers: CoverageTiers,
    /// Organization-derived remediation/help configuration (UF2-GEN seam).
    pub org_help: OrgHelpConfig,
}

impl DashboardConfig {
    /// Create a new `DashboardConfig` from threshold values.
    ///
    /// `org_help` starts fully generic (no organization-specific strings);
    /// set [`DashboardConfig::org_help`] directly to supply a deployment's
    /// own remediation guidance.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidValue`] if thresholds are out of range
    /// or `pass_threshold < warn_threshold`.
    pub fn new(pass_threshold: f64, warn_threshold: f64) -> Result<Self, ConfigError> {
        let tiers = CoverageTiers {
            pass_threshold,
            warn_threshold,
        };
        validate_tiers(&tiers)?;
        Ok(Self {
            tiers,
            org_help: OrgHelpConfig::default(),
        })
    }
}

/// Coverage tier thresholds for dashboard rendering.
///
/// Scores are classified into tiers:
/// - `>= pass_threshold` → Pass (green)
/// - `>= warn_threshold` and `< pass_threshold` → Warn (yellow)
/// - `< warn_threshold` → Fail (red)
///
/// When `pass_threshold == warn_threshold`, there is no warn band —
/// scores are either pass or fail. This is a valid configuration.
#[derive(Debug, Clone)]
pub struct CoverageTiers {
    /// Minimum percentage for "pass" tier (default: 80.0).
    pub pass_threshold: f64,
    /// Minimum percentage for "warn" tier (default: 50.0).
    pub warn_threshold: f64,
}

const DEFAULT_PASS_THRESHOLD: f64 = 80.0;
const DEFAULT_WARN_THRESHOLD: f64 = 50.0;

/// Default pass threshold value (80.0).
#[must_use]
pub const fn default_pass_threshold() -> f64 {
    DEFAULT_PASS_THRESHOLD
}

/// Default warn threshold value (50.0).
#[must_use]
pub const fn default_warn_threshold() -> f64 {
    DEFAULT_WARN_THRESHOLD
}

impl Default for CoverageTiers {
    fn default() -> Self {
        Self {
            pass_threshold: DEFAULT_PASS_THRESHOLD,
            warn_threshold: DEFAULT_WARN_THRESHOLD,
        }
    }
}

/// Validate that coverage tier thresholds are within bounds and ordered correctly.
fn validate_tiers(tiers: &CoverageTiers) -> Result<(), ConfigError> {
    if !(0.0..=100.0).contains(&tiers.pass_threshold) {
        return Err(ConfigError::InvalidValue {
            field: "pass-threshold".to_string(),
            reason: format!("must be in [0.0, 100.0], got {}", tiers.pass_threshold),
        });
    }
    if !(0.0..=100.0).contains(&tiers.warn_threshold) {
        return Err(ConfigError::InvalidValue {
            field: "warn-threshold".to_string(),
            reason: format!("must be in [0.0, 100.0], got {}", tiers.warn_threshold),
        });
    }
    if tiers.pass_threshold < tiers.warn_threshold {
        return Err(ConfigError::InvalidValue {
            field: "pass-threshold".to_string(),
            reason: format!(
                "pass-threshold ({}) must be >= warn-threshold ({})",
                tiers.pass_threshold, tiers.warn_threshold
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_correct_thresholds() {
        let config = DashboardConfig::default();
        assert!((config.tiers.pass_threshold - 80.0).abs() < f64::EPSILON);
        assert!((config.tiers.warn_threshold - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn new_config_validates_thresholds() {
        let config = DashboardConfig::new(90.0, 60.0).expect("valid thresholds");
        assert!((config.tiers.pass_threshold - 90.0).abs() < f64::EPSILON);
        assert!((config.tiers.warn_threshold - 60.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reject_pass_less_than_warn() {
        let err = DashboardConfig::new(40.0, 60.0).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("pass-threshold"), "got: {msg}");
    }

    #[test]
    fn reject_out_of_range() {
        let err = DashboardConfig::new(150.0, 50.0).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("pass-threshold"), "got: {msg}");
    }

    #[test]
    fn equal_thresholds_accepted() {
        let config = DashboardConfig::new(70.0, 70.0).expect("equal thresholds are valid");
        assert!((config.tiers.pass_threshold - 70.0).abs() < f64::EPSILON);
        assert!((config.tiers.warn_threshold - 70.0).abs() < f64::EPSILON);
    }
}
