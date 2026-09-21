use cherry_pit_core::Projection;
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersContent,
    CodeownersResult, DependabotResult, DependabotStatus, EnabledProvenance, ProbeSource,
    RepositoryChecks, SecretScanningAlerts, SecretScanningResult, SecurityPolicyResult,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};
use gh_report::event::DomainEvent;
use gh_report::projection::{EvidenceProjection, EvidenceProjectionEvent};
use pardosa::file::FileStorageAdapter;
use pardosa::prelude::*;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::time::Instant;

const EVENT_EPOCH_SECONDS: u64 = 1_726_130_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteMode {
    NativeFrames,
    DurableAppend,
}

impl WriteMode {
    fn label(self) -> &'static str {
        match self {
            WriteMode::NativeFrames => "native-frames",
            WriteMode::DurableAppend => "durable-append",
        }
    }
}

fn sample_claim(epoch: u64) -> OwnershipClaimRecord {
    OwnershipClaimRecord {
        epoch,
        machine_id: [0xaa; 16],
        boot_id: [0xbb; 16],
        process_id: 12345,
        process_start_time_ns: 1_700_000_000_000_000_000,
        claim_time_ns: 1_700_000_001_000_000_000,
        operator_label: "bench-projection".to_string(),
    }
}

fn event_nanos(seq: usize) -> u64 {
    (EVENT_EPOCH_SECONDS + seq as u64) * 1_000_000_000
}

fn observation_timestamp(seq: usize) -> String {
    let second = i64::try_from(EVENT_EPOCH_SECONDS + seq as u64).expect("observation second");
    jiff::Timestamp::from_second(second)
        .expect("observation second in range")
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn generate_evidence(repo_idx: usize, seq: usize) -> RepositoryEvidence {
    let name = format!("repo-{repo_idx}");
    let ts = observation_timestamp(seq);
    let sec_pass = !(repo_idx + seq).is_multiple_of(3);
    let bp_pass = seq.is_multiple_of(2);

    RepositoryEvidence {
        repository: Repository {
            id: format!("id-{name}"),
            node_id: None,
            name: name.clone(),
            visibility: if repo_idx.is_multiple_of(4) {
                Visibility::Private
            } else {
                Visibility::Public
            },
            language: Some("Rust".to_string()),
            default_branch: "main".to_string(),
            archived: false,
            has_issues: true,
            inventory_key: format!("org/{name}"),
            updated_at: None,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            is_empty: false,
            html_url: None,
            topics: vec![],
            license_spdx: Some("MIT".to_string()),
        },
        checks: RepositoryChecks {
            security_policy: if sec_pass {
                SecurityPolicyResult::EnabledBySetting {
                    timestamp: ts.clone(),
                }
            } else {
                SecurityPolicyResult::Absent {
                    timestamp: ts.clone(),
                }
            },
            secret_scanning: SecretScanningResult::Enabled {
                provenance: EnabledProvenance::Metadata {
                    http_status: None,
                    alerts: SecretScanningAlerts::Observable {
                        source: ProbeSource::PerRepoEndpoint,
                        has_open_alerts: false,
                        http_status: None,
                    },
                },
                timestamp: ts.clone(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: ts.clone(),
            },
            branch_protection: BranchProtectionResult {
                status: if bp_pass {
                    BranchProtectionStatus::Pass
                } else {
                    BranchProtectionStatus::Fail
                },
                details: BranchProtectionDetails {
                    default_branch: "main".to_string(),
                    has_pr: Some(true),
                    required_reviewers: Some(1),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                    reason_kind: None,
                    http_status: None,
                    force_push_blocked: Some(true),
                    deletion_blocked: Some(true),
                },
                timestamp: ts.clone(),
            },
            codeowners: CodeownersResult::Conforming {
                content: CodeownersContent::Unparsed,
                timestamp: ts,
            },
        },
        last_commit: None,
        repo_details_not_modified: false,
    }
}

fn domain_event(repo_idx: usize, seq: usize) -> DomainEvent {
    let native: gh_report::event::RepositoryEvidence = generate_evidence(repo_idx, seq)
        .try_into()
        .expect("native evidence");
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::new(format!("org/repo-{repo_idx}")).expect("domain key"),
        repo_name: NonEmptyEventString::new(format!("repo-{repo_idx}")).expect("repo name"),
        timestamp: Timestamp::new(event_nanos(seq)).expect("event timestamp"),
        evidence: Some(native),
    }
}

fn next_envelope(
    seq: usize,
    fibers: usize,
    heads: &mut HashMap<[u8; 16], EventEnvelope>,
    payload_bytes: &mut usize,
) -> EventEnvelope {
    let repo_idx = seq % fibers;
    let fiber_id = derive_fiber_id(&format!("org/repo-{repo_idx}"));
    let mut payload = Vec::new();
    domain_event(repo_idx, seq)
        .encode_payload(&mut payload)
        .expect("encode payload");
    *payload_bytes += payload.len();

    let event_id = *uuid::Uuid::now_v7().as_bytes();
    let envelope = match heads.get(&fiber_id) {
        Some(prev) => EventEnvelope::chain(prev, event_id, payload).expect("chain"),
        None => EventEnvelope::genesis(event_id, fiber_id, payload).expect("genesis"),
    };
    heads.insert(fiber_id, envelope.clone());
    envelope
}

fn build_store(
    adapter: &FileStorageAdapter,
    total: usize,
    fibers: usize,
    mode: WriteMode,
) -> usize {
    let descriptor = pardosa::prelude::AdmittedDescriptor::try_from_descriptor(
        pardosa::prelude::SchemaDescriptor::new(
            DomainEvent::SCHEMA_VERSION,
            DomainEvent::schema_descriptor(),
        ),
    )
    .expect("admit descriptor");
    let mut session = adapter
        .create(&sample_claim(1), &descriptor)
        .expect("create session");
    let mut heads: HashMap<[u8; 16], EventEnvelope> = HashMap::with_capacity(fibers);
    let mut payload_bytes = 0usize;

    match mode {
        WriteMode::DurableAppend => {
            for seq in 0..total {
                let envelope = next_envelope(seq, fibers, &mut heads, &mut payload_bytes);
                let verdict = session
                    .append_envelope_verdict(&envelope)
                    .expect("append envelope");
                assert!(
                    matches!(verdict, pardosa::store::WriteLandingVerdict::Landed(_)),
                    "durable append must land at seq {seq}"
                );
            }
            drop(session);
        }
        WriteMode::NativeFrames => {
            drop(session);
            let file = std::fs::OpenOptions::new()
                .append(true)
                .open(adapter.pgno_path())
                .expect("append-open created .pgno");
            let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
            for seq in 0..total {
                let envelope = next_envelope(seq, fibers, &mut heads, &mut payload_bytes);
                let mut envelope_bytes = Vec::new();
                envelope
                    .encode(&mut envelope_bytes)
                    .expect("encode envelope");
                let mut frame = Vec::new();
                ContainerFrame::encode_payload(&envelope_bytes, &mut frame).expect("encode frame");
                out.write_all(&frame).expect("write frame");
            }
            out.into_inner()
                .expect("flush store")
                .sync_data()
                .expect("sync store");
        }
    }

    assert_eq!(heads.len(), fibers.min(total), "retained head cardinality");
    payload_bytes
}

fn apply_envelope(projection: &mut EvidenceProjection, stored: &EventEnvelope) {
    let event = DomainEvent::decode_payload(&stored.payload).expect("decode domain event");
    let projection_event = match event {
        DomainEvent::RepositoryStateCaptured {
            domain_key,
            evidence,
            ..
        } => EvidenceProjectionEvent::RepositoryStateCaptured {
            detached: stored.header.detached,
            domain_key: domain_key.as_str().to_string(),
            evidence: evidence.map(|e| Box::new(e.into())),
        },
        DomainEvent::RepositoryDeleted {
            domain_key,
            repo_name,
            detected_at,
        } => EvidenceProjectionEvent::RepositoryDeleted {
            domain_key: domain_key.as_str().to_string(),
            repo_name: repo_name.as_str().to_string(),
            detected_at: detected_at.as_nanos().to_string(),
        },
        other => panic!("unexpected benchmark event: {other:?}"),
    };
    let envelope = cherry_pit_core::EventEnvelope::new(
        uuid::Uuid::now_v7(),
        cherry_pit_core::AggregateId::new(std::num::NonZeroU64::MIN),
        std::num::NonZeroU64::MIN,
        jiff::Timestamp::now(),
        None,
        None,
        projection_event,
    )
    .expect("projection envelope");
    projection.apply(&envelope);
}

fn project(envelopes: &[EventEnvelope]) -> EvidenceProjection {
    let mut projection = EvidenceProjection::default();
    for stored in envelopes {
        apply_envelope(&mut projection, stored);
    }
    projection
}

fn projection_digest(projection: &EvidenceProjection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let snapshot = projection.sorted_snapshot();
    snapshot.len().hash(&mut hasher);
    for item in &snapshot {
        item.repository.id.hash(&mut hasher);
        item.repository.name.hash(&mut hasher);
        (item.checks.security_policy.status() as u8).hash(&mut hasher);
        (item.checks.branch_protection.status as u8).hash(&mut hasher);
    }
    hasher.finish()
}

fn expected_latest_seq(total: usize, fibers: usize, repo_idx: usize) -> usize {
    let steps = (total - repo_idx - 1) / fibers;
    repo_idx + steps * fibers
}

fn assert_stored_chain_and_heads(envelopes: &[EventEnvelope], total: usize, fibers: usize) {
    let mut head_by_fiber: HashMap<[u8; 16], ([u8; 16], usize)> = HashMap::with_capacity(fibers);
    for (idx, stored) in envelopes.iter().enumerate() {
        if let Some((prev_event_id, prev_idx)) = head_by_fiber.get(&stored.header.fiber_id) {
            assert_eq!(
                stored.header.precursor, *prev_event_id,
                "same-fiber stored chain order violated at index {idx} (previous {prev_idx})"
            );
        }
        head_by_fiber.insert(stored.header.fiber_id, (stored.header.event_id, idx));
    }
    assert_eq!(head_by_fiber.len(), fibers, "stored fiber cardinality");

    for repo_idx in 0..fibers {
        let fiber_id = derive_fiber_id(&format!("org/repo-{repo_idx}"));
        let (_, head_idx) = head_by_fiber
            .get(&fiber_id)
            .unwrap_or_else(|| panic!("missing stored fiber for repo-{repo_idx}"));
        let expected_idx = expected_latest_seq(total, fibers, repo_idx);
        assert_eq!(
            *head_idx, expected_idx,
            "repo-{repo_idx} stored fiber head position"
        );
        match DomainEvent::decode_payload(&envelopes[*head_idx].payload).expect("decode head") {
            DomainEvent::RepositoryStateCaptured { timestamp, .. } => assert_eq!(
                timestamp.as_nanos(),
                event_nanos(expected_idx),
                "repo-{repo_idx} fiber head identity (alias-free per-event marker)"
            ),
            other => panic!("unexpected fiber head event: {other:?}"),
        }
    }
}

fn assert_expected_latest_state(projection: &EvidenceProjection, total: usize, fibers: usize) {
    let snapshot = projection.sorted_snapshot();
    assert_eq!(snapshot.len(), fibers, "projected fiber cardinality");
    let by_name: HashMap<String, _> = snapshot
        .iter()
        .map(|item| (item.repository.name.clone(), item))
        .collect();

    let mut observations = HashSet::with_capacity(fibers);
    for repo_idx in 0..fibers {
        let seq = expected_latest_seq(total, fibers, repo_idx);
        let expected = generate_evidence(repo_idx, seq);
        let name = format!("repo-{repo_idx}");
        let actual = by_name
            .get(&name)
            .unwrap_or_else(|| panic!("missing projected repository {name}"));

        assert_eq!(actual.repository.id, expected.repository.id, "{name} id");
        assert_eq!(
            format!("{:?}", actual.repository.visibility),
            format!("{:?}", expected.repository.visibility),
            "{name} visibility"
        );
        assert_eq!(
            format!("{:?}", actual.checks.security_policy.status()),
            format!("{:?}", expected.checks.security_policy.status()),
            "{name} security_policy status at seq {seq}"
        );
        assert_eq!(
            format!("{:?}", actual.checks.branch_protection.status),
            format!("{:?}", expected.checks.branch_protection.status),
            "{name} branch_protection status at seq {seq}"
        );
        assert_eq!(
            actual.checks.security_policy.timestamp(),
            expected.checks.security_policy.timestamp(),
            "{name} latest observation (staleness oracle) at seq {seq}"
        );
        assert_eq!(
            actual.checks.codeowners.timestamp(),
            expected.checks.codeowners.timestamp(),
            "{name} latest codeowners observation at seq {seq}"
        );
        assert_eq!(
            actual.checks.branch_protection.timestamp, expected.checks.branch_protection.timestamp,
            "{name} latest branch_protection observation at seq {seq}"
        );
        assert!(
            observations.insert(actual.checks.security_policy.timestamp().to_string()),
            "{name} projected observation must be alias-free at seq {seq}"
        );
    }

    assert_eq!(observations.len(), fibers, "projected observations aliased");
}

#[expect(
    clippy::cast_precision_loss,
    reason = "event counts are converted to f64 only to print throughput rates"
)]
fn run_stage(total: usize, fibers: usize, mode: WriteMode) {
    assert!(
        total / fibers >= 2,
        "stage must exercise update depth per fiber"
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let adapter = FileStorageAdapter::new(tmp.path().join(format!("bench_{total}.pgno")));

    let start_write = Instant::now();
    let payload_bytes = build_store(&adapter, total, fibers, mode);
    let write_secs = start_write.elapsed().as_secs_f64();
    let file_bytes = std::fs::metadata(adapter.pgno_path())
        .expect("store metadata")
        .len();
    println!(
        "\n--- [{}: {total} events, {fibers} fibers, depth {}] ---\nWRITE: {write_secs:.3}s => {:.0} ev/s (payload {payload_bytes} B, file {file_bytes} B)",
        mode.label(),
        total / fibers,
        (total as f64) / write_secs
    );

    let start_replay = Instant::now();
    let mut reader = adapter.open_read().expect("open_read");
    let envelopes = reader.read_all_envelopes().expect("read_all_envelopes");
    let read_secs = start_replay.elapsed().as_secs_f64();
    assert_eq!(envelopes.len(), total, "stored event count");
    assert_stored_chain_and_heads(&envelopes, total, fibers);

    let projection = project(&envelopes);
    let replay_secs = start_replay.elapsed().as_secs_f64();
    println!(
        "REPLAY: {replay_secs:.3}s => {:.0} ev/s (disk read {read_secs:.3}s)",
        (total as f64) / replay_secs
    );

    assert_eq!(projection.repositories.len(), fibers);
    assert_expected_latest_state(&projection, total, fibers);
    let warm_digest = projection_digest(&projection);
    let warm_commitment = reader.rolling_commitment().current_commitment();
    drop(projection);
    drop(envelopes);
    drop(reader);

    let mut cold_reader = adapter.open_read().expect("cold open_read");
    let cold_envelopes = cold_reader.read_all_envelopes().expect("cold read");
    assert_eq!(cold_envelopes.len(), total, "reopened event count");
    assert_eq!(
        cold_reader.rolling_commitment().current_commitment(),
        warm_commitment,
        "reopened rolling commitment must match"
    );
    let cold_projection = project(&cold_envelopes);
    assert_eq!(
        projection_digest(&cold_projection),
        warm_digest,
        "reopened projection digest must match warm digest"
    );
    assert_expected_latest_state(&cold_projection, total, fibers);
}

#[test]
fn projection_reconstructs_latest_state_at_scale() {
    run_stage(1_000, 20, WriteMode::NativeFrames);
    run_stage(10_000, 50, WriteMode::NativeFrames);
    if std::env::var("BENCH_SCALE").is_ok() {
        run_stage(100_000, 100, WriteMode::DurableAppend);
    }
}

#[test]
fn projection_reconstructs_latest_state_after_durable_append() {
    run_stage(200, 20, WriteMode::DurableAppend);
}

#[test]
fn projection_reconstructs_latest_state_1m() {
    if std::env::var("BENCH_1M").is_err() {
        eprintln!("Skipping 1M benchmark; run with BENCH_1M=1 to execute.");
        return;
    }
    run_stage(1_000_000, 200, WriteMode::DurableAppend);
}

#[test]
fn native_frame_store_is_readable_by_the_production_adapter() {
    run_stage(4, 2, WriteMode::NativeFrames);
}

#[test]
fn generated_observations_are_alias_free() {
    assert_ne!(
        generate_evidence(0, 9_950)
            .checks
            .security_policy
            .timestamp(),
        generate_evidence(0, 6_350)
            .checks
            .security_policy
            .timestamp(),
        "observation must distinguish the stale-by-3600 witness"
    );
    assert_ne!(
        event_nanos(9_950),
        event_nanos(6_350),
        "per-event envelope identity marker must distinguish the stale-by-3600 witness"
    );
    assert_eq!(
        generate_evidence(0, 0)
            .checks
            .security_policy
            .timestamp()
            .len(),
        generate_evidence(0, 999_999)
            .checks
            .security_policy
            .timestamp()
            .len(),
        "observation stays fixed-width"
    );
}

#[test]
fn generated_stream_covers_both_status_outcomes() {
    for (total, fibers) in [(1_000usize, 20usize), (10_000, 50)] {
        let mut security = HashSet::new();
        let mut protection = HashSet::new();
        let mut visibility = HashSet::new();
        for seq in 0..total {
            let evidence = generate_evidence(seq % fibers, seq);
            security.insert(format!("{:?}", evidence.checks.security_policy.status()));
            protection.insert(format!("{:?}", evidence.checks.branch_protection.status));
            visibility.insert(format!("{:?}", evidence.repository.visibility));
        }
        assert_eq!(security.len(), 2, "stream security_policy mix at {total}");
        assert_eq!(
            protection.len(),
            2,
            "stream branch_protection mix at {total}"
        );
        assert_eq!(visibility.len(), 2, "stream visibility mix at {total}");

        let mut head_security = HashSet::new();
        let mut head_protection = HashSet::new();
        for repo_idx in 0..fibers {
            let seq = expected_latest_seq(total, fibers, repo_idx);
            let evidence = generate_evidence(repo_idx, seq);
            head_security.insert(format!("{:?}", evidence.checks.security_policy.status()));
            head_protection.insert(format!("{:?}", evidence.checks.branch_protection.status));
        }
        assert_eq!(head_security.len(), 2, "fiber-head security mix at {total}");
        assert_eq!(
            head_protection.len(),
            2,
            "fiber-head branch_protection mix at {total}"
        );
    }
}

#[test]
fn generated_stream_covers_both_status_outcomes_at_tiny_scale() {
    let (total, fibers) = (4usize, 2usize);
    let heads: Vec<_> = (0..fibers)
        .map(|repo_idx| generate_evidence(repo_idx, expected_latest_seq(total, fibers, repo_idx)))
        .collect();
    let head_security: HashSet<String> = heads
        .iter()
        .map(|e| format!("{:?}", e.checks.security_policy.status()))
        .collect();
    let head_protection: HashSet<String> = heads
        .iter()
        .map(|e| format!("{:?}", e.checks.branch_protection.status))
        .collect();
    assert_eq!(
        head_security,
        HashSet::from(["Pass".to_string()]),
        "both tiny heads (seq 2, seq 3) pass security; the tiny stage is a format/read smoke test, not a status-mix scenario"
    );
    assert_eq!(
        head_protection.len(),
        2,
        "tiny heads still differ on branch_protection"
    );
}

#[test]
fn expected_latest_seq_is_the_last_occurrence_of_each_fiber() {
    for (total, fibers) in [(1_000usize, 20usize), (10_000, 50), (200, 20), (4, 2)] {
        for repo_idx in 0..fibers {
            let seq = expected_latest_seq(total, fibers, repo_idx);
            assert!(seq < total && seq % fibers == repo_idx);
            assert!(
                seq + fibers >= total,
                "expected_latest_seq({total},{fibers},{repo_idx}) is not the last occurrence"
            );
        }
    }
}
