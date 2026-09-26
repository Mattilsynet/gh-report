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

/// Non-shipping heap and RSS profiling harness (ghr-61c73290,
/// ghr-6946f6b2 memprof-01). Compiled only under the non-default
/// `profiling` feature; never active in a release build.
#[cfg(feature = "profiling")]
mod profiling {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum WaitOutcome {
        Stopped,
        TimedOut,
    }

    struct ShutdownSignal {
        stopped: Mutex<bool>,
        changed: Condvar,
    }

    impl ShutdownSignal {
        fn new() -> Self {
            Self {
                stopped: Mutex::new(false),
                changed: Condvar::new(),
            }
        }

        fn stop(&self) {
            let mut stopped = self
                .stopped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *stopped = true;
            self.changed.notify_all();
        }

        fn is_stopped(&self) -> bool {
            *self
                .stopped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn wait_timeout(&self, timeout: Duration) -> WaitOutcome {
            self.wait_timeout_enrolled(timeout, || {})
        }

        fn wait_timeout_enrolled(
            &self,
            timeout: Duration,
            on_enrolled: impl FnOnce(),
        ) -> WaitOutcome {
            let stopped = self
                .stopped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            on_enrolled();
            let (stopped, _) = self
                .changed
                .wait_timeout_while(stopped, timeout, |stopped| !*stopped)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *stopped {
                WaitOutcome::Stopped
            } else {
                WaitOutcome::TimedOut
            }
        }
    }

    /// RAII guard bundling the dhat heap profiler and the background RSS
    /// sampler. Dropping this at the end of `main` flushes `dhat-heap.json`
    /// and stops the sampler thread.
    pub struct ProfilingGuard {
        _dhat: dhat::Profiler,
        stop: std::sync::Arc<ShutdownSignal>,
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
            #[expect(
                clippy::unused_result_ok,
                reason = "profiling-only CSV header write is best effort; a failed write must not abort the profiled run"
            )]
            writeln!(csv, "epoch_ms,rss_bytes").ok();

            let stop = std::sync::Arc::new(ShutdownSignal::new());
            let stop_for_thread = std::sync::Arc::clone(&stop);
            let pid = std::process::id();
            let sampler = std::thread::spawn(move || {
                run_sampler(&stop_for_thread, SAMPLE_INTERVAL, move || {
                    if let Some(rss_bytes) = sample_rss_bytes(pid) {
                        let epoch_ms = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        #[expect(
                            clippy::unused_result_ok,
                            reason = "profiling-only RSS sample write is best effort; a failed write must not stop the sampler thread"
                        )]
                        writeln!(csv, "{epoch_ms},{rss_bytes}").ok();
                        #[expect(
                            clippy::unused_result_ok,
                            reason = "profiling-only CSV flush is best effort; a failed flush must not stop the sampler thread"
                        )]
                        csv.flush().ok();
                    }
                });
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
            self.stop.stop();
            if let Some(handle) = self.sampler.take() {
                #[expect(
                    clippy::unused_result_ok,
                    reason = "join result carries only the sampler thread panic payload; the Drop guard waits for the thread but must not panic while unwinding"
                )]
                handle.join().ok();
            }
        }
    }

    fn run_sampler(signal: &ShutdownSignal, interval: Duration, mut sample: impl FnMut()) {
        loop {
            if signal.is_stopped() {
                break;
            }
            sample();
            if signal.wait_timeout(interval) == WaitOutcome::Stopped {
                break;
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

    #[cfg(test)]
    mod tests {
        use super::{SAMPLE_INTERVAL, ShutdownSignal, WaitOutcome, run_sampler};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        #[test]
        fn sample_interval_stays_two_seconds() {
            assert_eq!(SAMPLE_INTERVAL, Duration::from_secs(2));
        }

        #[test]
        fn wait_times_out_when_no_stop_requested() {
            let signal = ShutdownSignal::new();
            let started = Instant::now();
            assert_eq!(
                signal.wait_timeout(Duration::from_millis(50)),
                WaitOutcome::TimedOut
            );
            assert!(started.elapsed() >= Duration::from_millis(50));
        }

        #[test]
        fn stop_before_wait_returns_stopped_without_waiting() {
            let signal = ShutdownSignal::new();
            signal.stop();
            let started = Instant::now();
            assert_eq!(
                signal.wait_timeout(Duration::from_secs(30)),
                WaitOutcome::Stopped
            );
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn stop_during_enrolled_wait_wakes_waiter() {
            let signal = Arc::new(ShutdownSignal::new());
            let stopper = Arc::clone(&signal);
            let handle = std::sync::Mutex::new(None);
            let started = Instant::now();
            let outcome = signal.wait_timeout_enrolled(Duration::from_secs(30), || {
                *handle.lock().expect("handle slot") = Some(std::thread::spawn(move || {
                    stopper.stop();
                }));
            });
            assert_eq!(outcome, WaitOutcome::Stopped);
            assert!(started.elapsed() < Duration::from_secs(5));
            handle
                .into_inner()
                .expect("handle slot")
                .expect("stopper spawned")
                .join()
                .expect("stopper thread joins");
        }

        #[test]
        fn pre_stopped_sampler_does_not_sample() {
            let signal = ShutdownSignal::new();
            signal.stop();
            let mut samples = 0_u32;
            run_sampler(&signal, Duration::from_secs(30), || samples += 1);
            assert_eq!(samples, 0);
        }

        #[test]
        fn active_sampler_samples_once_then_stops_on_signal() {
            let signal = Arc::new(ShutdownSignal::new());
            let stopper = Arc::clone(&signal);
            let mut samples = 0_u32;
            run_sampler(&signal, Duration::from_secs(30), || {
                samples += 1;
                stopper.stop();
            });
            assert_eq!(samples, 1);
        }

        #[test]
        fn repeated_stop_is_idempotent() {
            let signal = ShutdownSignal::new();
            signal.stop();
            signal.stop();
            assert_eq!(
                signal.wait_timeout(Duration::from_secs(30)),
                WaitOutcome::Stopped
            );
        }
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

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum RateRegulatorArg {
    #[default]
    TokenBucket,
    BudgetGate,
}

impl From<RateRegulatorArg> for runtime::RateRegulatorKind {
    fn from(value: RateRegulatorArg) -> Self {
        match value {
            RateRegulatorArg::TokenBucket => Self::TokenBucket,
            RateRegulatorArg::BudgetGate => Self::BudgetGate,
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

    /// Rollback seam for the roster-eda render cutover (ghr-a3091aef):
    /// force the render path back to the pre-cutover synchronous
    /// `collect_team_rosters` live fetch instead of reading the persisted
    /// projection. Default off — the projection read is the shipped path.
    #[arg(long, env = "GH_REPORT_TEAM_ROSTER_LIVE_FETCH")]
    team_roster_live_fetch: bool,

    /// Run as a read-only serving replica: do not run background collection or team refresh loops.
    #[arg(long, alias = "read-only", env = "GH_REPORT_SERVE_ONLY")]
    serve_only: bool,

    /// Persistent store directory for baseline, checkpoints, and lock files.
    #[arg(long, default_value = "store")]
    store_dir: PathBuf,

    /// Pardosa backend for the event log.
    #[arg(long, default_value = "pgno", env = "GH_REPORT_PARDOSA_BACKEND")]
    pardosa_backend: PardosaBackendArg,

    /// Primary-rate `Regulator` for the worker-pool regulator chain
    /// (ghr-79f5d695 kill-switch): `token-bucket` (default) or
    /// `budget-gate` (operational fallback, revert via config alone).
    #[arg(long, default_value = "token-bucket", env = "GH_REPORT_RATE_REGULATOR")]
    rate_regulator: RateRegulatorArg,

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

    /// Maximum distinct repositories admitted per collection sweep.
    #[arg(long, default_value_t = config::MaxRepos::default(), env = "GH_REPORT_MAX_REPOS")]
    max_repos: config::MaxRepos,

    /// Minimum coverage percentage for the "pass" tier (green).
    #[arg(long, default_value_t = dashboard::default_pass_threshold())]
    pass_threshold: f64,

    /// Minimum coverage percentage for the "warn" tier (yellow).
    #[arg(long, default_value_t = dashboard::default_warn_threshold())]
    warn_threshold: f64,
}

#[expect(
    clippy::too_many_lines,
    reason = "synchronous root runtime supervision, CLI dispatch, and daemon initialization"
)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(feature = "profiling")]
    let _profiling_guard = profiling::ProfilingGuard::start();

    gh_report::infra::tls::install_default_crypto_provider();

    let cli = Cli::parse();

    let nats_runtime = match cli.pardosa_backend {
        PardosaBackendArg::Nats => Some(std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("ghr-nats-rt")
                .build()?,
        )),
        PardosaBackendArg::Pgno => None,
    };

    let app_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ghr-app-rt")
        .build()?;

    let root_nats_guard = nats_runtime.as_ref().map(std::sync::Arc::clone);

    let result = app_runtime.block_on(async move {
        if cli.dump_baseline {
            let org = cli.org.as_deref().ok_or(
                "--org is required when using --dump-baseline (δ.3c-ii: event/projection stores are per-org)",
            )?;
            let events_dir = cli.store_dir.join("events").join(org);
            let app_state = gh_report::app::state::AppState::with_stores_and_runtime(
                &events_dir,
                runtime::PardosaBackend::from(cli.pardosa_backend),
                runtime::NatsStoreConfig::for_org(org, cli.nats_url.clone())?
                    .with_credentials_path(cli.nats_creds.clone()),
                root_nats_guard.clone(),
            )
            .await?;
            if let Err(e) = app_state.snapshot_fast_path_init() {
                return Err(format!("projection init failed: {e}").into());
            }
            match app_state.dump_baseline_json() {
                Ok(json) => {
                    println!("{json}");
                    return Ok(());
                }
                Err(e) => {
                    return Err(format!("serialise baseline: {e}").into());
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

        let dashboard_config =
            dashboard::DashboardConfig::new(cli.pass_threshold, cli.warn_threshold)?;
        let mut config = runtime::RuntimeConfig::with_force_unlock(
            org,
            cli.no_resume,
            cli.max_workers,
            cli.store_dir,
            cli.force_unlock,
            dashboard_config,
        )?;
        config.pardosa_backend = runtime::PardosaBackend::from(cli.pardosa_backend);
        config.rate_regulator = runtime::RateRegulatorKind::from(cli.rate_regulator);
        config.nats_url = cli.nats_url;
        config.nats_creds = cli.nats_creds;
        config.force_refresh = cli.force_refresh;
        config.team_roster_read_from_projection = !cli.team_roster_live_fetch;
        config.nats_runtime = root_nats_guard;
        config.max_repos = cli.max_repos;
        config.serve_only = cli.serve_only;
        let nats_creds_path = config
            .nats_creds
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let sanitized_nats_url =
            gh_report::app::state::sanitize_nats_url(&config.nats_url);
        tracing::info!(
            org = %config.org_name,
            backend = ?config.pardosa_backend,
            nats_url = %sanitized_nats_url,
            nats_creds_path = %nats_creds_path,
            "effective startup config"
        );
        gh_report::app::daemon::run(config).await?;

        Ok(())
    });

    drop(app_runtime);
    drop(nats_runtime);

    result
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
    fn cli_default_rate_regulator_is_token_bucket() {
        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();
        assert!(matches!(cli.rate_regulator, RateRegulatorArg::TokenBucket));
    }

    #[test]
    fn cli_parses_rate_regulator_budget_gate() {
        let cli = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--rate-regulator",
            "budget-gate",
        ])
        .unwrap();
        assert!(matches!(cli.rate_regulator, RateRegulatorArg::BudgetGate));
    }

    #[test]
    fn cli_rejects_invalid_rate_regulator() {
        let result = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--rate-regulator",
            "bogus",
        ]);
        assert!(result.is_err());
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

    #[test]
    fn cli_default_max_repos_is_one_thousand() {
        let cli = Cli::try_parse_from(["gh-report", "--org", "test-org"]).unwrap();
        assert_eq!(cli.max_repos.get(), 1000);
    }

    #[test]
    fn cli_parses_custom_max_repos() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--max-repos", "250"]).unwrap();
        assert_eq!(cli.max_repos.get(), 250);
    }

    #[test]
    fn cli_parses_ten_max_repos() {
        let cli =
            Cli::try_parse_from(["gh-report", "--org", "test-org", "--max-repos", "10"]).unwrap();
        assert_eq!(cli.max_repos.get(), 10);
    }

    #[test]
    fn cli_rejects_zero_max_repos() {
        let result = Cli::try_parse_from(["gh-report", "--org", "test-org", "--max-repos", "0"]);
        assert!(result.is_err());
    }

    #[test]
    fn cli_rejects_invalid_max_repos() {
        let result = Cli::try_parse_from([
            "gh-report",
            "--org",
            "test-org",
            "--max-repos",
            "notanumber",
        ]);
        assert!(result.is_err());
    }
}
