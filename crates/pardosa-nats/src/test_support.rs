//! Live-NATS test harness (cfg-gated by `feature = "test-support"`).
//!
//! [`LiveNatsServer`] is a test-only RAII guard: asserts the
//! `nats-server` on `PATH` matches `tools/.nats-server-version`,
//! binds an ephemeral port, spawns the binary with a per-spawn
//! [`tempfile::TempDir`] for `JetStream` state, awaits readiness,
//! exposes the spawned URL via [`LiveNatsServer::url`], and reaps
//! on [`Drop`].
//!
//! [`LiveNatsServer::acquire`] is a `Mutex<Weak<Self>>` singleton —
//! first caller spawns, later callers share. Test consumers thread
//! the URL through [`crate::JetStreamConfigBuilder::nats_url`].
//!
//! Canonical home, single-sourced across `pardosa-nats`, `pardosa`,
//! `pardosa-test-support-harness`.
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;
const READINESS_BUDGET: Duration = Duration::from_secs(10);
const READINESS_INITIAL_BACKOFF: Duration = Duration::from_millis(25);
const READINESS_BACKOFF_CAP: Duration = Duration::from_millis(500);
/// Owning handle for a single spawned `nats-server` plus its
/// per-instance `JetStream` tempdir.
///
/// Constructed via [`Self::acquire`]. Holding an
/// [`Arc<LiveNatsServer>`] guarantees the server is reachable at
/// [`Self::url`]; releasing the last `Arc` triggers
/// [`Drop::drop`], which kills and reaps the child process and
/// removes the tempdir.
pub struct LiveNatsServer {
    url: String,
    _tempdir: TempDir,
    child: Mutex<Option<Child>>,
}
impl LiveNatsServer {
    /// Return a shared handle to a running, JetStream-enabled
    /// `nats-server`. First caller spawns (asserting the binary
    /// version matches `tools/.nats-server-version`); concurrent
    /// callers share. Threads the spawned URL through the returned
    /// [`Arc`]; consumers reach it via [`Self::url`] and feed it
    /// into [`crate::JetStreamConfigBuilder::nats_url`].
    ///
    /// # Panics
    ///
    /// Panics if the `nats-server` binary version mismatches the
    /// pin, cannot be invoked, no ephemeral port can be bound, or
    /// readiness times out. Panic is the correct failure mode —
    /// a misconfigured workstation must not silently run live
    /// tests against an unintended server.
    #[must_use]
    pub fn acquire() -> Arc<Self> {
        match Self::try_acquire() {
            LiveNats::Ready(server) => server,
            LiveNats::Unavailable(reason) => {
                panic!("spawn nats-server for test harness: {reason}")
            }
            LiveNats::Fatal(error) => {
                panic!("spawn nats-server for test harness: {error}")
            }
        }
    }
    /// Fallible sibling of [`Self::acquire`], sharing the same
    /// `Mutex<Weak<Self>>` singleton and the same version pin check.
    ///
    /// Separates the two skippable conditions — an absent `nats-server`
    /// executable and a mismatch against `tools/.nats-server-version` —
    /// from every other harness startup fault. Only the former yield
    /// [`LiveNats::Unavailable`]; a failed port bind, tempdir, spawn, or
    /// readiness wait yields [`LiveNats::Fatal`], which no caller may
    /// turn into a passing test.
    ///
    /// # Panics
    ///
    /// Panics only if the singleton mutex is poisoned. A missing or
    /// mismatched `nats-server` is reported as
    /// [`LiveNats::Unavailable`], not as a panic.
    #[must_use]
    pub fn try_acquire() -> LiveNats {
        static SINGLETON: OnceLock<Mutex<Weak<LiveNatsServer>>> = OnceLock::new();
        let cell = SINGLETON.get_or_init(|| Mutex::new(Weak::new()));
        let mut guard = cell
            .lock()
            .expect("LiveNatsServer singleton mutex poisoned");
        if let Some(existing) = guard.upgrade() {
            return LiveNats::Ready(existing);
        }
        match Self::spawn() {
            Ok(fresh) => {
                let fresh = Arc::new(fresh);
                *guard = Arc::downgrade(&fresh);
                LiveNats::Ready(fresh)
            }
            Err(StartupFault::Skippable(reason)) => LiveNats::Unavailable(reason),
            Err(StartupFault::Fatal(error)) => LiveNats::Fatal(error),
        }
    }
    /// URL of the spawned server in `nats://<host>:<port>` form.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
    fn spawn() -> Result<Self, StartupFault> {
        assert_version_pinned()?;
        let port = reserve_ephemeral_port()?;
        let tempdir =
            TempDir::new().map_err(|source| StartupFault::from(HarnessError::TempDir(source)))?;
        let host = "127.0.0.1";
        let child = Command::new("nats-server")
            .arg("-a")
            .arg(host)
            .arg("-p")
            .arg(port.to_string())
            .arg("-js")
            .arg("-sd")
            .arg(tempdir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| StartupFault::from(HarnessError::Spawn(source)))?;
        let url = format!("nats://{host}:{port}");
        wait_for_readiness(&url)?;
        Ok(Self {
            url,
            _tempdir: tempdir,
            child: Mutex::new(Some(child)),
        })
    }
}
impl Drop for LiveNatsServer {
    fn drop(&mut self) {
        let Ok(mut guard) = self.child.lock() else {
            return;
        };
        let Some(mut child) = guard.take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }
}
/// Outcome of [`LiveNatsServer::try_acquire`]: a live harness, a
/// legitimately skippable absence, or a fatal harness startup fault.
///
/// The three-way split is load-bearing. Only [`Self::Unavailable`] —
/// an absent `nats-server` executable or a version mismatch against
/// the pin — may be turned into a skipped test. Every other startup
/// fault is [`Self::Fatal`] and must fail the test, so a broken
/// harness on a correctly-pinned runner cannot silently erase
/// assertions while still reporting green.
#[non_exhaustive]
pub enum LiveNats {
    Ready(Arc<LiveNatsServer>),
    Unavailable(Unavailable),
    Fatal(HarnessError),
}
impl LiveNats {
    /// Unwrap to a live handle, or emit a skip line to stderr naming
    /// `test_name` and the reason, and return [`None`].
    ///
    /// Call sites use it as an early return, which keeps the test
    /// compiled into the binary and executed whenever the pinned
    /// server IS present — unlike an ignore attribute, which would
    /// take the test dark on every runner that does not pass
    /// `--include-ignored`.
    ///
    /// [`None`] means, and can only mean, [`Self::Unavailable`]: the
    /// signature admits no path from a fatal harness fault to a
    /// passing early return.
    ///
    /// # Panics
    ///
    /// Panics on [`Self::Fatal`]. A failed port bind, tempdir, spawn,
    /// or readiness wait is a broken harness rather than an absent
    /// one, and must fail the test instead of skipping it.
    #[must_use]
    pub fn ready_or_skip(self, test_name: &str) -> Option<Arc<LiveNatsServer>> {
        match self {
            Self::Ready(server) => Some(server),
            Self::Unavailable(reason) => {
                eprintln!("SKIP {test_name}: live nats-server unavailable: {reason}");
                None
            }
            Self::Fatal(error) => {
                panic!("live nats-server harness failed to start for {test_name}: {error}")
            }
        }
    }
}
/// The only two conditions under which a live-NATS test may legitimately
/// skip: the `nats-server` executable is absent from `PATH`, or the one
/// present does not match `tools/.nats-server-version`.
///
/// Deliberately disjoint from [`HarnessError`] — an infrastructure fault
/// has no representation here, so it cannot reach a skip path.
#[derive(Debug)]
#[non_exhaustive]
pub enum Unavailable {
    ExecutableAbsent(std::io::Error),
    VersionMismatch { expected: String, observed: String },
}
impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExecutableAbsent(source) => {
                write!(f, "no nats-server executable on PATH: {source}")
            }
            Self::VersionMismatch { expected, observed } => {
                write!(
                    f,
                    "nats-server version mismatch: pinned={expected}, observed={observed}; \
                 install the pinned version or update tools/.nats-server-version"
                )
            }
        }
    }
}
impl std::error::Error for Unavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExecutableAbsent(source) => Some(source),
            Self::VersionMismatch { .. } => None,
        }
    }
}
enum StartupFault {
    Skippable(Unavailable),
    Fatal(HarnessError),
}
impl From<HarnessError> for StartupFault {
    fn from(error: HarnessError) -> Self {
        Self::Fatal(error)
    }
}
impl From<Unavailable> for StartupFault {
    fn from(reason: Unavailable) -> Self {
        Self::Skippable(reason)
    }
}
/// A fatal live-NATS harness startup fault.
///
/// Every variant means the harness itself is broken on a machine that
/// was supposed to be able to run it. Absence and version drift are
/// NOT represented here; they live in [`Unavailable`].
#[derive(Debug)]
#[non_exhaustive]
pub enum HarnessError {
    VersionFile {
        path: PathBuf,
        source: std::io::Error,
    },
    VersionProbe(std::io::Error),
    Bind(std::io::Error),
    TempDir(std::io::Error),
    Spawn(std::io::Error),
    NotReady {
        url: String,
        attempts: u32,
        last_error: String,
    },
}
impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionFile { path, source } => {
                write!(
                    f,
                    "cannot read pinned nats-server version from {}: {source}",
                    path.display()
                )
            }
            Self::VersionProbe(source) => {
                write!(f, "cannot invoke nats-server --version: {source}")
            }
            Self::Bind(source) => {
                write!(f, "cannot reserve ephemeral port for nats-server: {source}")
            }
            Self::TempDir(source) => {
                write!(f, "cannot create JetStream tempdir: {source}")
            }
            Self::Spawn(source) => write!(f, "cannot spawn nats-server child: {source}"),
            Self::NotReady {
                url,
                attempts,
                last_error,
            } => {
                write!(
                    f,
                    "nats-server did not accept a TCP connection at {url} within \
                 {READINESS_BUDGET:?} ({attempts} attempts); last connect error: {last_error}",
                )
            }
        }
    }
}
impl std::error::Error for HarnessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::VersionFile { source, .. }
            | Self::VersionProbe(source)
            | Self::Bind(source)
            | Self::TempDir(source)
            | Self::Spawn(source) => Some(source),
            Self::NotReady { .. } => None,
        }
    }
}
fn assert_version_pinned() -> Result<(), StartupFault> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pin_path = manifest_dir
        .join("..")
        .join("..")
        .join("tools")
        .join(".nats-server-version");
    let pinned = std::fs::read_to_string(&pin_path)
        .map_err(|source| {
            StartupFault::from(HarnessError::VersionFile {
                path: pin_path,
                source,
            })
        })?
        .trim()
        .to_string();
    let output = Command::new("nats-server")
        .arg("--version")
        .output()
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                StartupFault::from(Unavailable::ExecutableAbsent(source))
            } else {
                StartupFault::from(HarnessError::VersionProbe(source))
            }
        })?;
    let observed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let needle = format!("v{pinned}");
    if !observed.split_whitespace().any(|tok| tok == needle) {
        return Err(StartupFault::from(Unavailable::VersionMismatch {
            expected: pinned,
            observed,
        }));
    }
    Ok(())
}
fn reserve_ephemeral_port() -> Result<u16, StartupFault> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|source| StartupFault::from(HarnessError::Bind(source)))?;
    let port = listener
        .local_addr()
        .map_err(|source| StartupFault::from(HarnessError::Bind(source)))?
        .port();
    drop(listener);
    Ok(port)
}
fn wait_for_readiness(url: &str) -> Result<(), StartupFault> {
    let rt = Runtime::new().map_err(|source| {
        StartupFault::from(HarnessError::NotReady {
            url: url.to_string(),
            attempts: 0,
            last_error: format!("cannot start tokio runtime for readiness probe: {source}"),
        })
    })?;
    let start = Instant::now();
    let mut attempts: u32 = 0;
    let mut backoff = READINESS_INITIAL_BACKOFF;
    let mut last_error = String::from("no attempts made");
    while start.elapsed() < READINESS_BUDGET {
        attempts = attempts.saturating_add(1);
        match rt.block_on(async_nats::connect(url)) {
            Ok(client) => {
                let _ = rt.block_on(async move { client.flush().await });
                return Ok(());
            }
            Err(e) => {
                last_error = e.to_string();
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(READINESS_BACKOFF_CAP);
            }
        }
    }
    Err(StartupFault::from(HarnessError::NotReady {
        url: url.to_string(),
        attempts,
        last_error,
    }))
}
#[cfg(test)]
mod tests {
    use super::{HarnessError, LiveNats, Unavailable};
    #[test]
    fn fatal_harness_fault_does_not_skip() {
        let outcome = LiveNats::Fatal(HarnessError::TempDir(std::io::Error::other(
            "injected tempdir fault",
        )));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            outcome.ready_or_skip("injected")
        }));
        assert!(
            result.is_err(),
            "a fatal harness startup fault must fail the test, not skip it"
        );
    }
    #[test]
    fn version_mismatch_still_skips() {
        let outcome = LiveNats::Unavailable(Unavailable::VersionMismatch {
            expected: "2.14.5".to_string(),
            observed: "nats-server: v2.14.6".to_string(),
        });
        assert!(
            outcome.ready_or_skip("injected").is_none(),
            "a version mismatch must remain skippable"
        );
    }
    #[test]
    fn absent_executable_still_skips() {
        let outcome = LiveNats::Unavailable(Unavailable::ExecutableAbsent(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
        assert!(
            outcome.ready_or_skip("injected").is_none(),
            "an absent nats-server executable must remain skippable"
        );
    }
    #[test]
    fn source_bearing_harness_errors_expose_their_cause() {
        use std::error::Error as _;
        let with_source = HarnessError::Spawn(std::io::Error::other("boom"));
        assert!(with_source.source().is_some());
        let without_source = HarnessError::NotReady {
            url: "nats://127.0.0.1:1".to_string(),
            attempts: 3,
            last_error: "refused".to_string(),
        };
        assert!(without_source.source().is_none());
    }
}
