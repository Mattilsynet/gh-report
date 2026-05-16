#![forbid(unsafe_code)]
// TODO(P0a, bd-adr-fmt-5f8s): doc_markdown — 55 mechanical backtick adds.
// Deferred: SM2 was abandoned (automaton over-correction). Revisit via a
// hand-written per-comment pass.
#![allow(clippy::doc_markdown)]

//! `gh-report` CLI entrypoint.
//!
//! Thin binary that wires commands, config, and logging. All business
//! logic lives in the library crate.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use gh_report::config::{self, dashboard, runtime};

/// Log output format.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogFormat {
    /// Human-readable, colored output (default).
    #[default]
    Text,
    /// Structured JSON lines — suitable for log aggregation pipelines.
    Json,
}

#[derive(Parser)]
#[command(
    name = "gh-report",
    about = "GitHub organization governance collector and reporter",
    version
)]
struct Cli {
    /// Log output format.
    #[arg(
        long,
        global = true,
        default_value = "text",
        env = "GH_REPORT_LOG_FORMAT"
    )]
    log_format: LogFormat,

    /// Target GitHub organization name.
    #[arg(long)]
    org: Option<String>,

    /// Do not reuse any existing checkpoint file.
    #[arg(long)]
    no_resume: bool,

    /// Forcibly remove an existing lock before acquiring.
    /// Applies to the initial collection only (one-shot).
    /// WARNING: may break a genuinely concurrent run.
    #[arg(long)]
    force_unlock: bool,

    /// Persistent store directory for baseline, checkpoints, and lock files.
    #[arg(long, default_value = "store")]
    store_dir: PathBuf,

    /// Dump the baseline file as JSON to stdout and exit.
    #[arg(long)]
    dump_baseline: bool,

    /// Number of concurrent repository workers.
    #[arg(long, default_value_t = config::DEFAULT_MAX_WORKERS)]
    max_workers: usize,

    /// Minimum coverage percentage for the "pass" tier (green).
    #[arg(long, default_value_t = dashboard::default_pass_threshold())]
    pass_threshold: f64,

    /// Minimum coverage percentage for the "warn" tier (yellow).
    #[arg(long, default_value_t = dashboard::default_warn_threshold())]
    warn_threshold: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Handle --dump-baseline before initializing tracing (pure stdout output).
    if cli.dump_baseline {
        match gh_report::infra::baseline::dump_baseline(&cli.store_dir) {
            Ok(json) => {
                println!("{json}");
                return Ok(());
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }

    let org = cli
        .org
        .as_deref()
        .ok_or("--org is required when running the daemon")?;

    // Initialize tracing — format is chosen before any other work.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    match cli.log_format {
        LogFormat::Text => {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
        }
        LogFormat::Json => {
            use tracing_subscriber::layer::SubscriberExt;

            let cloud_logging = gh_report::infra::cloud_logging::CloudLoggingLayer::new();
            let subscriber = tracing_subscriber::Registry::default()
                .with(env_filter)
                .with(cloud_logging);
            tracing::subscriber::set_global_default(subscriber)
                .expect("failed to set global subscriber");
        }
    }

    let dashboard_config = dashboard::DashboardConfig::new(cli.pass_threshold, cli.warn_threshold)?;
    let config = runtime::RuntimeConfig::with_force_unlock(
        org,
        cli.no_resume,
        cli.max_workers,
        cli.store_dir,
        cli.force_unlock,
        dashboard_config,
    )?;
    gh_report::app::daemon::run(config).await?;

    Ok(())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_log_format_text() {
        let cli = Cli::try_parse_from(["gh-report", "--log-format", "text", "--org", "test-org"])
            .unwrap();
        assert!(matches!(cli.log_format, LogFormat::Text));
    }

    #[test]
    fn cli_parses_log_format_json() {
        let cli = Cli::try_parse_from(["gh-report", "--log-format", "json", "--org", "test-org"])
            .unwrap();
        assert!(matches!(cli.log_format, LogFormat::Json));
    }

    #[test]
    fn cli_default_log_format_is_text() {
        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();
        assert!(matches!(cli.log_format, LogFormat::Text));
    }

    #[test]
    fn cli_rejects_invalid_log_format() {
        let result = Cli::try_parse_from(["gh-report", "--log-format", "xml", "--org", "test-org"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_log_format_works_as_global_option() {
        let cli =
            Cli::try_parse_from(["gh-report", "--log-format", "json", "--org", "test"]).unwrap();
        assert!(matches!(cli.log_format, LogFormat::Json));
    }

    #[test]
    fn cli_requires_org_or_dump_baseline() {
        // Without --org or --dump-baseline, parse still succeeds
        // (org is Optional), but main() would fail at runtime.
        let cli = Cli::try_parse_from(["gh-report"]).unwrap();
        assert!(cli.org.is_none());
        assert!(!cli.dump_baseline);
    }

    #[test]
    fn cli_parses_dump_baseline() {
        let cli = Cli::try_parse_from(["gh-report", "--dump-baseline"]).unwrap();
        assert!(cli.dump_baseline);
    }

    #[test]
    fn cli_parses_force_unlock() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--force-unlock"]).unwrap();
        assert!(cli.force_unlock);
    }

    #[test]
    fn cli_default_thresholds() {
        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();
        assert!((cli.pass_threshold - 80.0).abs() < f64::EPSILON);
        assert!((cli.warn_threshold - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cli_default_store_dir() {
        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();
        assert_eq!(cli.store_dir, std::path::PathBuf::from("store"));
    }

    #[test]
    fn cli_custom_store_dir() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--store-dir", "/data/gh"])
                .unwrap();
        assert_eq!(cli.store_dir, std::path::PathBuf::from("/data/gh"));
    }

    #[test]
    fn cli_custom_thresholds() {
        let cli = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--pass-threshold",
            "90.0",
            "--warn-threshold",
            "60.0",
        ])
        .unwrap();
        assert!((cli.pass_threshold - 90.0).abs() < f64::EPSILON);
        assert!((cli.warn_threshold - 60.0).abs() < f64::EPSILON);
    }
}
