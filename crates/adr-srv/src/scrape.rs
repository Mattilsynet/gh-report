//! Scrape pipeline — walks the ADR corpus via the AFM-0026:R1 library
//! surface and projects each `adr_fmt::AdrRecord` into an `AdrIngested`
//! event, ingested via [`AdrService::ingest_if_changed`] for body-hash
//! idempotency. See [`scrape_corpus`] for the discovery + walk contract.
//!
//! Per AFM-0027:R3, adr-srv RE-PROJECTS rather than re-exporting.
//! Records missing any of `title` / `date` / `last_reviewed` / `tier` /
//! `status` are SKIPPED and reported in `ScrapeReport.diagnostics`
//! rather than fabricated with sentinel values — `body_hash`
//! idempotency is meaningless for events whose payload was synthesised.
//!
//! `references` preserves source order including duplicates (filtered
//! to `verb == RelVerb::References`); pinned by
//! `tests/scrape_pipeline.rs::references_preserve_order_and_duplicates`.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use adr_fmt::{
    AdrRecord, Config, DomainDir, LoadError, RelVerb, Status as AdrFmtStatus, Tier as AdrFmtTier,
    load_quiet, parse_domain, parse_stale, resolve_corpus_root,
};

use crate::app::{AdrService, IngestOutcome};
use crate::domain::adr_date::AdrDate;
use crate::domain::adr_id::AdrId;
use crate::domain::body_hash::BodyHash;
use crate::domain::events::AdrIngested;
use crate::domain::frontmatter::{AdrFrontmatter, Status, Tier};
use crate::projection::AdrCorpus;

/// Summary of one scrape pass.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScrapeReport {
    /// Total number of `AdrRecord`s walked (whether or not they
    /// emitted an event).
    pub records_seen: usize,
    /// Number of new events emitted by `ingest_if_changed`
    /// (`Created` + `Appended`). Idempotent re-scrape on an unchanged
    /// corpus emits 0.
    pub events_emitted: usize,
    /// Per-record skip / projection diagnostics. One entry per record
    /// that was walked but not ingested (missing frontmatter field,
    /// unparseable id, unreadable file, etc.).
    pub diagnostics: Vec<String>,
}

/// Error variants for [`scrape_corpus`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ScrapeError {
    /// `adr-fmt.toml` could not be loaded.
    Config(LoadError),
    /// Corpus root could not be resolved from the marker.
    ResolveCorpus(String),
    /// `parse_domain` / `parse_stale` returned `Err` (infrastructure
    /// failure: unreadable directory).
    Parse(String),
    /// `AdrService::ingest_if_changed` surfaced a `StoreError`.
    Store(cherry_pit_core::StoreError),
    /// `fs::read` on an ADR file failed (computed `body_hash` requires
    /// raw bytes per AFM-0027:R4).
    Io(std::io::Error),
}

impl core::fmt::Display for ScrapeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Config(e) => write!(f, "scrape: config load failed: {e}"),
            Self::ResolveCorpus(s) => write!(f, "scrape: resolve corpus root failed: {s}"),
            Self::Parse(s) => write!(f, "scrape: parse failed: {s}"),
            Self::Store(e) => write!(f, "scrape: store error: {e}"),
            Self::Io(e) => write!(f, "scrape: io error: {e}"),
        }
    }
}

impl std::error::Error for ScrapeError {}

impl From<LoadError> for ScrapeError {
    fn from(e: LoadError) -> Self {
        Self::Config(e)
    }
}

impl From<cherry_pit_core::StoreError> for ScrapeError {
    fn from(e: cherry_pit_core::StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<std::io::Error> for ScrapeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Walk the corpus rooted at `marker_dir`, project every parseable
/// ADR record into an `AdrIngested` event, and ingest via
/// `service.ingest_if_changed`. Returns a [`ScrapeReport`].
///
/// # Errors
/// Surfaces `ScrapeError` on infrastructure failures (config load,
/// corpus resolve, directory read, file read, store append). Per-
/// record projection failures (missing frontmatter, unparseable id)
/// are recorded in `ScrapeReport.diagnostics` and the walk continues.
pub async fn scrape_corpus(
    service: &AdrService,
    marker_dir: &Path,
    corpus: &Arc<Mutex<AdrCorpus>>,
) -> Result<ScrapeReport, ScrapeError> {
    let config: Config = load_quiet(marker_dir)?;
    let corpus_root =
        resolve_corpus_root(marker_dir, &config.corpus).map_err(ScrapeError::ResolveCorpus)?;

    let mut report = ScrapeReport::default();

    for domain in &config.domains {
        let dir = DomainDir {
            path: corpus_root.join(&domain.directory),
            prefix: domain.prefix.clone(),
            name: domain.name.clone(),
        };
        if !dir.path.is_dir() {
            report.diagnostics.push(format!(
                "domain directory missing: {} (prefix {})",
                dir.path.display(),
                domain.prefix
            ));
            continue;
        }
        let outcome = parse_domain(&dir).map_err(ScrapeError::Parse)?;
        for record in outcome.records {
            ingest_record(service, &record, &mut report, corpus).await?;
        }
        for diag in outcome.diagnostics {
            report.diagnostics.push(format!("parser: {}", diag.message));
        }
    }

    let stale_dir = corpus_root.join(&config.stale.directory);
    if stale_dir.is_dir() {
        let outcome = parse_stale(&stale_dir, &config).map_err(ScrapeError::Parse)?;
        for record in outcome.records {
            ingest_record(service, &record, &mut report, corpus).await?;
        }
        for diag in outcome.diagnostics {
            report.diagnostics.push(format!("parser: {}", diag.message));
        }
    }

    Ok(report)
}

/// Project a single `AdrRecord` into an `AdrIngested` event and
/// ingest. Updates `report` with `records_seen` + `events_emitted`
/// counters and per-record skip diagnostics. Returns `Err` only on
/// infrastructure failure (IO read, store error).
async fn ingest_record(
    service: &AdrService,
    record: &AdrRecord,
    report: &mut ScrapeReport,
    corpus: &Arc<Mutex<AdrCorpus>>,
) -> Result<(), ScrapeError> {
    report.records_seen += 1;

    let projected = match project(record) {
        Ok(event) => event,
        Err(reason) => {
            report
                .diagnostics
                .push(format!("skip {}: {reason}", record.id));
            return Ok(());
        }
    };

    match service.ingest_if_changed(projected, corpus).await? {
        IngestOutcome::Created | IngestOutcome::Appended => {
            report.events_emitted += 1;
        }
        IngestOutcome::Unchanged => {}
    }

    Ok(())
}

/// Project an `AdrRecord` into an `AdrIngested` event. Returns `Err`
/// with a human-readable reason when the record is incomplete; the
/// caller turns that into a `report.diagnostics` entry.
fn project(record: &AdrRecord) -> Result<AdrIngested, String> {
    let id: AdrId = record
        .id
        .to_string()
        .parse()
        .map_err(|e| format!("unparseable id ({e})"))?;

    let title = record
        .title
        .clone()
        .ok_or_else(|| "missing title".to_string())?;

    let date = parse_civil_date(
        record
            .date
            .as_deref()
            .ok_or_else(|| "missing date".to_string())?,
    )
    .map_err(|e| format!("invalid date: {e}"))?;

    let last_reviewed = parse_civil_date(
        record
            .last_reviewed
            .as_deref()
            .ok_or_else(|| "missing last_reviewed".to_string())?,
    )
    .map_err(|e| format!("invalid last_reviewed: {e}"))?;

    let tier = record
        .tier
        .map(project_tier)
        .ok_or_else(|| "missing tier".to_string())?;

    let status = project_status(
        record
            .status
            .as_ref()
            .ok_or_else(|| "missing status".to_string())?,
    );

    let body_bytes = fs::read(&record.file_path)
        .map_err(|e| format!("read file {}: {e}", record.file_path.display()))?;
    let body_hash = BodyHash::compute(&body_bytes);

    let references: Vec<AdrId> = record
        .relationships
        .iter()
        .filter(|r| r.verb == RelVerb::References)
        .map(|r| {
            r.target
                .to_string()
                .parse::<AdrId>()
                .map_err(|e| format!("unparseable reference {}: {e}", r.target))
        })
        .collect::<Result<_, _>>()?;

    Ok(AdrIngested {
        id,
        frontmatter: AdrFrontmatter {
            title,
            date,
            last_reviewed,
            tier,
            status,
        },
        body_hash,
        references,
    })
}

/// Parse a `YYYY-MM-DD` calendar date.
fn parse_civil_date(s: &str) -> Result<AdrDate, String> {
    let trimmed = s.trim();
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("expected YYYY-MM-DD, got {trimmed:?}"));
    }
    let year: i16 = parts[0].parse().map_err(|e| format!("year parse: {e}"))?;
    let month: u8 = parts[1].parse().map_err(|e| format!("month parse: {e}"))?;
    let day: u8 = parts[2].parse().map_err(|e| format!("day parse: {e}"))?;
    AdrDate::new(year, month, day).map_err(|e| e.to_string())
}

/// adr-fmt `Tier` → adr-srv `Tier`. Wire discriminants are pinned
/// locally per `crates/adr-srv/src/domain/frontmatter.rs`.
fn project_tier(t: AdrFmtTier) -> Tier {
    match t {
        AdrFmtTier::S => Tier::S,
        AdrFmtTier::A => Tier::A,
        AdrFmtTier::B => Tier::B,
        AdrFmtTier::C => Tier::C,
        AdrFmtTier::D => Tier::D,
    }
}

/// adr-fmt `Status` → adr-srv `Status`. The `SupersededBy(target)`
/// payload variant is collapsed to bare `Superseded`; the supersedes
/// target is captured by a separate `AdrSuperseded` event in Phase 3
/// per `domain/frontmatter.rs` rationale (no payload state on this
/// discriminant). `Invalid(raw)` → `Status::Invalid`.
fn project_status(s: &AdrFmtStatus) -> Status {
    match s {
        AdrFmtStatus::Draft => Status::Draft,
        AdrFmtStatus::Proposed => Status::Proposed,
        AdrFmtStatus::Accepted => Status::Accepted,
        AdrFmtStatus::Rejected => Status::Rejected,
        AdrFmtStatus::Deprecated => Status::Deprecated,
        AdrFmtStatus::SupersededBy(_) => Status::Superseded,
        AdrFmtStatus::Invalid(_) => Status::Invalid,
    }
}
