#![forbid(unsafe_code)]
use clap::{Parser, Subcommand, ValueEnum};
use pardosa::store::{EventStore, PgnoBackend};
use pardosa_cli::DomainEvent;
use pardosa_cli::event::limits::{
    MAX_BATCH_ID, MAX_DOMAIN_KEY, MAX_ERROR_MESSAGE, MAX_EVIDENCE, MAX_ORG, MAX_REPO_NAME,
    MAX_SNAPSHOT_SIG, MAX_SOURCE,
};
use pardosa_schema::{EventString, NonEmptyEventString, Timestamp};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
const DEFAULT_STORE: &str = "./pardosa-cli.pgno";
#[derive(Parser, Debug)]
#[command(
    name = "pardosa-cli",
    version,
    about = "DomainEvent .pgno CLI over pardosa::store (ADR-0018)",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}
#[derive(Subcommand, Debug)]
enum Cmd {
    SweepStart {
        #[arg(long)]
        org: String,
        #[arg(long)]
        batch: String,
        #[arg(long)]
        repos: u64,
        #[arg(long)]
        snapshot_signature: Option<String>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: PathBuf,
    },
    RepoEvaluated {
        #[arg(long)]
        repo: String,
        #[arg(long, value_enum)]
        outcome: Outcome,
        #[arg(long)]
        duration_ms: u64,
        #[arg(long)]
        evidence: Option<String>,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: PathBuf,
    },
    RepoRemoved {
        #[arg(long)]
        key: String,
        #[arg(long)]
        repo: String,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: PathBuf,
    },
    SweepComplete {
        #[arg(long)]
        batch: String,
        #[arg(long)]
        duration_ms: u64,
        #[arg(long, default_value_t = 0)]
        repos: u64,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: PathBuf,
    },
    SweepFailed {
        #[arg(long)]
        batch: String,
        #[arg(long)]
        error: String,
        #[arg(long)]
        duration_ms: u64,
        #[arg(long, default_value = DEFAULT_STORE)]
        store: PathBuf,
    },
    Read {
        path: PathBuf,
        #[arg(long)]
        from: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Inspect {
        path: PathBuf,
    },
}
#[derive(Copy, Clone, Debug, ValueEnum)]
enum Outcome {
    Success,
    Failure,
}
impl Outcome {
    fn as_bool(self) -> bool {
        matches!(self, Outcome::Success)
    }
}
fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pardosa-cli: {e}");
            ExitCode::from(1)
        }
    }
}
fn run(cli: Cli) -> Result<(), String> {
    match cli.cmd {
        Cmd::SweepStart {
            org,
            batch,
            repos,
            snapshot_signature,
            store,
        } => {
            let event = DomainEvent::SweepStarted {
                org: to_nes::<MAX_ORG>(&org, "org")?,
                repo_count: repos,
                batch_id: to_nes::<MAX_BATCH_ID>(&batch, "batch")?,
                timestamp: wall_clock_ts()?,
                snapshot_signature: opt_to_es::<MAX_SNAPSHOT_SIG>(
                    snapshot_signature,
                    "snapshot_signature",
                )?,
            };
            append_event(&store, event)
        }
        Cmd::RepoEvaluated {
            repo,
            outcome,
            duration_ms,
            evidence,
            store,
        } => {
            let repo_key = to_nes::<MAX_DOMAIN_KEY>(&repo, "repo")?;
            let repo_name = to_nes::<MAX_REPO_NAME>(&repo, "repo")?;
            let event = DomainEvent::RepoEvaluated {
                domain_key: repo_key,
                repo_name,
                success: outcome.as_bool(),
                source: to_nes::<MAX_SOURCE>("cli", "source")?,
                duration_ms,
                timestamp: wall_clock_ts()?,
                evidence: opt_to_es::<MAX_EVIDENCE>(evidence, "evidence")?,
            };
            append_event(&store, event)
        }
        Cmd::RepoRemoved { key, repo, store } => {
            let event = DomainEvent::RepoRemoved {
                domain_key: to_nes::<MAX_DOMAIN_KEY>(&key, "key")?,
                repo_name: to_nes::<MAX_REPO_NAME>(&repo, "repo")?,
                timestamp: wall_clock_ts()?,
            };
            append_event(&store, event)
        }
        Cmd::SweepComplete {
            batch,
            duration_ms,
            repos,
            store,
        } => {
            let event = DomainEvent::SweepCompleted {
                batch_id: to_nes::<MAX_BATCH_ID>(&batch, "batch")?,
                duration_ms,
                repo_count: repos,
                timestamp: wall_clock_ts()?,
            };
            append_event(&store, event)
        }
        Cmd::SweepFailed {
            batch,
            error,
            duration_ms,
            store,
        } => {
            let event = DomainEvent::SweepFailed {
                batch_id: to_nes::<MAX_BATCH_ID>(&batch, "batch")?,
                error: to_es::<MAX_ERROR_MESSAGE>(error, "error")?,
                duration_ms,
                timestamp: wall_clock_ts()?,
            };
            append_event(&store, event)
        }
        Cmd::Read { path, from, limit } => cmd_read(&path, from, limit),
        Cmd::Inspect { path } => cmd_inspect(&path),
    }
}
fn append_event(store_path: &Path, event: DomainEvent) -> Result<(), String> {
    let mut store: EventStore<DomainEvent> = if store_path.exists() {
        EventStore::<DomainEvent>::open_with_backend(PgnoBackend::open(store_path))
            .map_err(|e| format!("open {}: {e}", store_path.display()))?
    } else {
        EventStore::<DomainEvent>::create(store_path)
            .map_err(|e| format!("create {}: {e}", store_path.display()))?
    };
    let mut writer = store.writer();
    let _ = writer.begin(event).map_err(|e| format!("begin: {e}"))?;
    let _lsn = writer.sync().map_err(|e| format!("sync: {e}"))?;
    Ok(())
}
fn cmd_read(path: &Path, from: Option<usize>, limit: Option<usize>) -> Result<(), String> {
    let store: EventStore<DomainEvent> =
        EventStore::<DomainEvent>::open_with_backend(PgnoBackend::open(path))
            .map_err(|e| format!("open {}: {e}", path.display()))?;
    let reader = store.reader();
    let sidecar_dir = tempfile::tempdir().map_err(|e| format!("tempdir for read sidecar: {e}"))?;
    let sidecar = sidecar_dir.path().join("read.sidecar");
    let mut cursor = reader
        .cursor(&sidecar)
        .map_err(|e| format!("cursor open: {e}"))?;
    let from = from.unwrap_or(0);
    let limit = limit.unwrap_or(usize::MAX);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut yielded = 0usize;
    for (idx, ev) in cursor.tail().enumerate() {
        if idx < from {
            continue;
        }
        if yielded >= limit {
            break;
        }
        let ev = ev.map_err(|e| format!("tail at {idx}: {e}"))?;
        let id = ev.event_id();
        writeln!(out, "{id:?} {:#?}", ev.into_inner()).map_err(|e| format!("stdout: {e}"))?;
        writeln!(out).map_err(|e| format!("stdout: {e}"))?;
        yielded += 1;
    }
    Ok(())
}
fn cmd_inspect(path: &Path) -> Result<(), String> {
    let meta = EventStore::<DomainEvent>::metadata(path).map_err(|e| format!("metadata: {e}"))?;
    println!("schema_hash = 0x{:032X}", meta.schema_hash());
    println!("schema_source = {:?}", meta.schema_source());
    println!("message_count = {}", meta.len());
    println!(
        "note: page_class / schema_size / index_entries are not exposed by pardosa::store \
         (ADR-0018 § Naming sole-interface seal); inspect via the substrate crate or extend \
         StoreMetadata in a follow-up if required."
    );
    Ok(())
}
fn to_nes<const MAX: usize>(s: &str, field: &str) -> Result<NonEmptyEventString<MAX>, String> {
    NonEmptyEventString::<MAX>::try_new(s)
        .map_err(|_| format!("--{field}: empty or exceeds {MAX} bytes"))
}
fn to_es<const MAX: usize>(s: String, field: &str) -> Result<EventString<MAX>, String> {
    EventString::<MAX>::try_from(s).map_err(|_| format!("--{field}: exceeds {MAX} bytes"))
}
fn opt_to_es<const MAX: usize>(
    s: Option<String>,
    field: &str,
) -> Result<Option<EventString<MAX>>, String> {
    s.map(|v| to_es::<MAX>(v, field)).transpose()
}
fn wall_clock_ts() -> Result<Timestamp, String> {
    let nanos_u128 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock before UNIX epoch: {e}"))?
        .as_nanos();
    let nanos = u64::try_from(nanos_u128).map_err(|_| "nanos overflow u64".to_string())?;
    Timestamp::from_nanos(nanos).ok_or_else(|| "zero-nanos timestamp rejected".to_string())
}
