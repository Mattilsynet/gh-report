#![forbid(unsafe_code)]

//! `gh-report` CLI entrypoint.
//!
//! Thin binary that wires commands, config, and logging. All business
//! logic lives in the library crate.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use gh_report::config::{self, dashboard, runtime};

#[cfg(feature = "profiling")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(not(any(feature = "profiling", target_env = "msvc")))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(not(any(feature = "profiling", target_env = "msvc")))]
fn log_jemalloc_config() {
    match tikv_jemalloc_ctl::opt::background_thread::read() {
        Ok(background_thread) => tracing::info!(
            background_thread,
            "jemalloc allocator active; effective malloc_conf confirmed via opt.background_thread"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            "jemalloc allocator active; could not read opt.background_thread to confirm malloc_conf"
        ),
    }
}

#[cfg(any(feature = "profiling", target_env = "msvc"))]
fn log_jemalloc_config() {}

/// Non-shipping heap and RSS profiling harness (adr-fmt-gcuq4,
/// adr-fmt-nfteo memprof-01). Compiled only under the non-default
/// `profiling` feature; never active in a release build.
#[cfg(feature = "profiling")]
mod profiling {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// RAII guard bundling the dhat heap profiler and the background RSS
    /// sampler. Dropping this at the end of `main` flushes `dhat-heap.json`
    /// and stops the sampler thread.
    pub struct ProfilingGuard {
        _dhat: dhat::Profiler,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        sampler: Option<std::thread::JoinHandle<()>>,
    }

    impl ProfilingGuard {
        /// Starts the dhat heap profiler and an RSS-over-time sampler.
        ///
        /// # Panics
        ///
        /// Panics if the RSS CSV path (env `RSS_CSV`, default `rss.csv`)
        /// cannot be created.
        #[must_use]
        pub fn start() -> Self {
            let dhat_profiler = dhat::Profiler::builder()
                .file_name(
                    std::env::var("DHAT_HEAP_JSON")
                        .unwrap_or_else(|_| "dhat-heap.json".to_string()),
                )
                .build();

            let csv_path = std::env::var("RSS_CSV").unwrap_or_else(|_| "rss.csv".to_string());
            let mut csv = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&csv_path)
                .expect("open RSS_CSV path for the profiling harness");
            writeln!(csv, "epoch_ms,rss_bytes").ok();

            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_for_thread = std::sync::Arc::clone(&stop);
            let pid = std::process::id();
            let sampler = std::thread::spawn(move || {
                while !stop_for_thread.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Some(rss_bytes) = sample_rss_bytes(pid) {
                        let epoch_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        writeln!(csv, "{epoch_ms},{rss_bytes}").ok();
                        csv.flush().ok();
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            });

            Self {
                _dhat: dhat_profiler,
                stop,
                sampler: Some(sampler),
            }
        }
    }

    impl Drop for ProfilingGuard {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(handle) = self.sampler.take() {
                handle.join().ok();
            }
        }
    }

    /// Samples the resident set size of `pid` in bytes via `ps -o rss=`.
    ///
    /// Returns `None` if the `ps` invocation fails or its output cannot be
    /// parsed; the sampler simply skips that tick.
    fn sample_rss_bytes(pid: u32) -> Option<u64> {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let rss_kb: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        Some(rss_kb * 1024)
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogFormat {
    /// Human-readable, colored output (default).
    #[default]
    Text,
    /// Structured JSON lines — suitable for log aggregation pipelines.
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PardosaBackendArg {
    Pgno,
    Nats,
}

impl From<PardosaBackendArg> for runtime::PardosaBackend {
    fn from(value: PardosaBackendArg) -> Self {
        match value {
            PardosaBackendArg::Pgno => Self::Pgno,
            PardosaBackendArg::Nats => Self::Nats,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "gh-report",
    about = "GitHub organization governance collector and reporter",
    version = env!("GH_REPORT_VERSION")
)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "CLI struct mirrors operator --flags 1:1; each bool is an independent switch, and collapsing them would obscure the flag surface"
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

    /// Bypass baseline reuse for the initial collection, re-fetching every
    /// repository. Applies to the initial collection only (one-shot).
    #[arg(long, env = "GH_REPORT_FORCE_REFRESH")]
    force_refresh: bool,

    /// Persistent store directory for baseline, checkpoints, and lock files.
    #[arg(long, default_value = "store")]
    store_dir: PathBuf,

    /// Pardosa backend for the event log.
    #[arg(long, default_value = "pgno", env = "GH_REPORT_PARDOSA_BACKEND")]
    pardosa_backend: PardosaBackendArg,

    /// NATS server URL for the pardosa Nats backend.
    #[arg(long, default_value = runtime::DEFAULT_NATS_URL, env = "GH_REPORT_NATS_URL")]
    nats_url: String,

    /// Filesystem path to a NATS .creds file for the pardosa Nats backend.
    #[arg(long, env = "GH_REPORT_NATS_CREDS")]
    nats_creds: Option<PathBuf>,

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

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "profiling")]
    let _profiling_guard = profiling::ProfilingGuard::start();

    gh_report::infra::tls::install_default_crypto_provider();

    let cli = Cli::parse();

    if cli.dump_baseline {
        let org = cli.org.as_deref().ok_or(
            "--org is required when using --dump-baseline (δ.3c-ii: event/projection stores are per-org)",
        )?;
        let events_dir = cli.store_dir.join("events").join(org);
        let app_state = gh_report::app::state::AppState::with_stores(
            &events_dir,
            runtime::PardosaBackend::from(cli.pardosa_backend),
            runtime::NatsStoreConfig::for_org(org, cli.nats_url.clone())?
                .with_credentials_path(cli.nats_creds.clone()),
        )
        .await?;
        if let Err(e) = app_state.snapshot_fast_path_init() {
            eprintln!("error: projection init failed: {e}");
            std::process::exit(1);
        }
        match app_state.dump_baseline_json() {
            Ok(json) => {
                println!("{json}");
                return Ok(());
            }
            Err(e) => {
                eprintln!("error: serialise baseline: {e}");
                std::process::exit(1);
            }
        }
    }

    let org = cli
        .org
        .as_deref()
        .ok_or("--org is required when running the daemon")?;

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

    log_jemalloc_config();

    let dashboard_config = dashboard::DashboardConfig::new(cli.pass_threshold, cli.warn_threshold)?;
    let mut config = runtime::RuntimeConfig::with_force_unlock(
        org,
        cli.no_resume,
        cli.max_workers,
        cli.store_dir,
        cli.force_unlock,
        dashboard_config,
    )?;
    config.pardosa_backend = runtime::PardosaBackend::from(cli.pardosa_backend);
    config.nats_url = cli.nats_url;
    config.nats_creds = cli.nats_creds;
    config.force_refresh = cli.force_refresh;
    let nats_creds_path = config
        .nats_creds
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    tracing::info!(
        org = %config.org_name,
        backend = ?config.pardosa_backend,
        nats_url = %config.nats_url,
        nats_creds_path = %nats_creds_path,
        "effective startup config"
    );
    gh_report::app::daemon::run(config).await?;

    Ok(())
}

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
    fn cli_parses_force_refresh() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--force-refresh"]).unwrap();
        assert!(cli.force_refresh);
    }

    #[test]
    fn cli_parses_force_refresh_env() {
        const CHILD_ENV: &str = "GH_REPORT_FORCE_REFRESH_ENV_CHILD";

        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("cli_parses_force_refresh_env")
                .arg("--exact")
                .env(CHILD_ENV, "1")
                .env("GH_REPORT_FORCE_REFRESH", "true")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child test failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();

        assert!(cli.force_refresh);
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
    fn cli_parses_pardosa_backend() {
        let cli = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--pardosa-backend",
            "nats",
        ])
        .unwrap();
        assert!(matches!(
            runtime::PardosaBackend::from(cli.pardosa_backend),
            runtime::PardosaBackend::Nats
        ));
    }

    #[test]
    fn cli_parses_nats_url() {
        let cli = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--pardosa-backend",
            "nats",
            "--nats-url",
            "nats://127.0.0.1:4223",
        ])
        .unwrap();

        assert_eq!(cli.nats_url, "nats://127.0.0.1:4223");
    }

    #[test]
    fn cli_parses_nats_creds_flag() {
        let cli = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--pardosa-backend",
            "nats",
            "--nats-creds",
            "/var/secrets/nats.creds",
        ])
        .unwrap();

        assert_eq!(
            cli.nats_creds,
            Some(PathBuf::from("/var/secrets/nats.creds"))
        );
    }

    #[test]
    fn cli_parses_nats_creds_env() {
        const CHILD_ENV: &str = "GH_REPORT_NATS_CREDS_ENV_CHILD";
        let path = PathBuf::from("/var/secrets/nats.creds");

        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("cli_parses_nats_creds_env")
                .arg("--exact")
                .env(CHILD_ENV, "1")
                .env("GH_REPORT_NATS_CREDS", &path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child test failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();

        assert_eq!(cli.nats_creds, Some(path));
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

    #[test]
    fn cli_dump_baseline_parses() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--dump-baseline"]).unwrap();
        assert!(cli.dump_baseline);
    }
}
