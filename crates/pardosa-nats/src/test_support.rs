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
    pub fn acquire() -> Arc<Self> {
        static SINGLETON: OnceLock<Mutex<Weak<LiveNatsServer>>> = OnceLock::new();
        let cell = SINGLETON.get_or_init(|| Mutex::new(Weak::new()));
        let mut guard = cell
            .lock()
            .expect("LiveNatsServer singleton mutex poisoned");
        guard.upgrade().unwrap_or_else(|| {
            let fresh = Arc::new(Self::spawn().expect("spawn nats-server for test harness"));
            *guard = Arc::downgrade(&fresh);
            fresh
        })
    }
    /// URL of the spawned server in `nats://<host>:<port>` form.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
    fn spawn() -> Result<Self, HarnessError> {
        assert_version_pinned()?;
        let port = reserve_ephemeral_port()?;
        let tempdir = TempDir::new().map_err(HarnessError::TempDir)?;
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
            .map_err(HarnessError::Spawn)?;
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
#[derive(Debug)]
enum HarnessError {
    VersionFile {
        path: PathBuf,
        source: std::io::Error,
    },
    VersionProbe(std::io::Error),
    VersionMismatch {
        expected: String,
        observed: String,
    },
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
            Self::VersionMismatch { expected, observed } => {
                write!(
                    f,
                    "nats-server version mismatch: pinned={expected}, observed={observed}; \
                 install the pinned version or update tools/.nats-server-version"
                )
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
impl std::error::Error for HarnessError {}
fn assert_version_pinned() -> Result<(), HarnessError> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pin_path = manifest_dir
        .join("..")
        .join("..")
        .join("tools")
        .join(".nats-server-version");
    let pinned = std::fs::read_to_string(&pin_path)
        .map_err(|source| HarnessError::VersionFile {
            path: pin_path,
            source,
        })?
        .trim()
        .to_string();
    let output = Command::new("nats-server")
        .arg("--version")
        .output()
        .map_err(HarnessError::VersionProbe)?;
    let observed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let needle = format!("v{pinned}");
    if !observed.split_whitespace().any(|tok| tok == needle) {
        return Err(HarnessError::VersionMismatch {
            expected: pinned,
            observed,
        });
    }
    Ok(())
}
fn reserve_ephemeral_port() -> Result<u16, HarnessError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(HarnessError::Bind)?;
    let port = listener.local_addr().map_err(HarnessError::Bind)?.port();
    drop(listener);
    Ok(port)
}
fn wait_for_readiness(url: &str) -> Result<(), HarnessError> {
    let rt = Runtime::new().map_err(|source| HarnessError::NotReady {
        url: url.to_string(),
        attempts: 0,
        last_error: format!("cannot start tokio runtime for readiness probe: {source}"),
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
    Err(HarnessError::NotReady {
        url: url.to_string(),
        attempts,
        last_error,
    })
}
