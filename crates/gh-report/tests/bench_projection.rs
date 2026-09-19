#![allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use cherry_pit_core::Projection;
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};
use gh_report::event::DomainEvent;
use gh_report::projection::{EvidenceProjection, EvidenceProjectionEvent};
use pardosa::file::FileStorageAdapter;
use pardosa::prelude::*;
use std::hash::{Hash, Hasher};
use std::time::Instant;

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

fn generate_evidence(repo_idx: usize, seq: usize) -> RepositoryEvidence {
    let name = format!("repo-{repo_idx}");
    let ts = format!("2026-09-12T10:{:02}:{:02}Z", (seq / 60) % 60, seq % 60);
    let sec_pass = !(repo_idx + seq).is_multiple_of(3);
    let bp_pass = (repo_idx + seq).is_multiple_of(2);

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
            security_policy: SecurityPolicyResult {
                status: if sec_pass {
                    SecurityPolicyStatus::Pass
                } else {
                    SecurityPolicyStatus::Fail
                },
                evidence: SecurityPolicyEvidence::Setting,
                path: None,
                timestamp: ts.clone(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
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
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: ts,
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
        repo_details_not_modified: false,
    }
}

fn apply_projection_event(projection: &mut EvidenceProjection, event: EvidenceProjectionEvent) {
    let envelope = match cherry_pit_core::EventEnvelope::new(
        uuid::Uuid::now_v7(),
        cherry_pit_core::AggregateId::new(std::num::NonZeroU64::MIN),
        std::num::NonZeroU64::MIN,
        jiff::Timestamp::now(),
        None,
        None,
        event,
    ) {
        Ok(envelope) => envelope,
        Err(error) => panic!("projection envelope invariant violated: {error}"),
    };
    projection.apply(&envelope);
}

fn apply_event_to_projection(
    projection: &mut EvidenceProjection,
    event: &DomainEvent,
    detached: bool,
) {
    match event {
        DomainEvent::RepositoryStateCaptured {
            domain_key,
            evidence,
            ..
        } => {
            apply_projection_event(
                projection,
                EvidenceProjectionEvent::RepositoryStateCaptured {
                    detached,
                    domain_key: domain_key.as_str().to_string(),
                    evidence: evidence.as_ref().map(|e| Box::new((*e).clone().into())),
                },
            );
        }
        DomainEvent::RepositoryDeleted {
            domain_key,
            repo_name,
            detected_at,
        } => {
            apply_projection_event(
                projection,
                EvidenceProjectionEvent::RepositoryDeleted {
                    domain_key: domain_key.as_str().to_string(),
                    repo_name: repo_name.as_str().to_string(),
                    detected_at: detected_at.as_nanos().to_string(),
                },
            );
        }
        _ => {}
    }
}

fn compute_projection_digest(projection: &EvidenceProjection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let snapshot = projection.sorted_snapshot();
    snapshot.len().hash(&mut hasher);
    for item in &snapshot {
        item.repository.id.hash(&mut hasher);
        item.repository.name.hash(&mut hasher);
        (item.checks.security_policy.status as u8).hash(&mut hasher);
        (item.checks.branch_protection.status as u8).hash(&mut hasher);
    }
    hasher.finish()
}

fn run_staged_benchmark(total_events: usize, batch_size: usize, distinct_repos: usize) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store_path = tmp.path().join(format!("bench_{total_events}.pgno"));
    let adapter = FileStorageAdapter::new(&store_path);

    let claim = sample_claim(1);
    let mut session = adapter.create(&claim).expect("create session");

    println!(
        "\n--- [STAGE: {total_events} events, batch_size: {batch_size}, fibers: {distinct_repos}] ---"
    );

    let start_write = Instant::now();
    let mut events_written = 0usize;
    let mut bytes_written = 0usize;

    let mut latest_env_by_fiber: std::collections::HashMap<[u8; 16], EventEnvelope> =
        std::collections::HashMap::with_capacity(distinct_repos);

    for chunk_start in (0..total_events).step_by(batch_size) {
        let chunk_end = (chunk_start + batch_size).min(total_events);
        let chunk_len = chunk_end - chunk_start;
        let mut batch_envelopes = Vec::with_capacity(chunk_len);

        for i in chunk_start..chunk_end {
            let repo_idx = i % distinct_repos;
            let domain_key = format!("org/repo-{repo_idx}");
            let fiber_id = derive_fiber_id(&domain_key);
            let evidence = generate_evidence(repo_idx, i);
            let native_evidence: gh_report::event::RepositoryEvidence =
                evidence.try_into().expect("try_into native evidence");

            let domain_event = DomainEvent::RepositoryStateCaptured {
                domain_key: NonEmptyEventString::new(domain_key).unwrap(),
                repo_name: NonEmptyEventString::new(format!("repo-{repo_idx}")).unwrap(),
                timestamp: Timestamp::new((1_726_130_000 + i as u64) * 1_000_000_000).unwrap(),
                evidence: Some(native_evidence),
            };

            let mut payload = Vec::new();
            domain_event
                .encode_payload(&mut payload)
                .expect("encode payload");
            bytes_written += payload.len();

            let event_id = *uuid::Uuid::now_v7().as_bytes();
            let env = if let Some(prev) = latest_env_by_fiber.get(&fiber_id) {
                EventEnvelope::chain(prev, event_id, payload).expect("chain")
            } else {
                EventEnvelope::genesis(event_id, fiber_id, payload).expect("genesis")
            };
            latest_env_by_fiber.insert(fiber_id, env.clone());
            batch_envelopes.push(env);
        }

        for env in batch_envelopes {
            let verdict = session
                .append_envelope_verdict(&env)
                .expect("append envelope");
            assert!(matches!(
                verdict,
                pardosa::store::WriteLandingVerdict::Landed(_)
            ));
            events_written += 1;
        }
    }

    let write_duration = start_write.elapsed();
    let write_secs = write_duration.as_secs_f64();
    let write_throughput = (events_written as f64) / write_secs;
    let write_mb_s = ((bytes_written as f64) / (1024.0 * 1024.0)) / write_secs;

    println!(
        "WRITE: {events_written} events in {write_secs:.3}s => {write_throughput:.0} ev/s ({write_mb_s:.2} MB/s)"
    );

    let start_replay = Instant::now();
    let mut reader = adapter.open_read().expect("open_read");
    let envelopes = reader.read_all_envelopes().expect("read_all_envelopes");
    let read_all_duration = start_replay.elapsed();
    assert_eq!(envelopes.len(), total_events);

    let mut projection = EvidenceProjection::default();
    let start_apply = Instant::now();
    let mut decode_micros_total = 0u128;

    for env in &envelopes {
        let t0 = Instant::now();
        let domain_event = DomainEvent::decode_payload(&env.payload).expect("decode domain event");
        decode_micros_total += t0.elapsed().as_micros();
        apply_event_to_projection(&mut projection, &domain_event, env.header.detached);
    }

    let apply_duration = start_apply.elapsed();
    let total_replay_duration = start_replay.elapsed();
    let replay_secs = total_replay_duration.as_secs_f64();
    let replay_throughput = (total_events as f64) / replay_secs;
    let decode_avg_us = (decode_micros_total as f64) / (total_events as f64);
    let apply_avg_us = (apply_duration.as_micros() as f64) / (total_events as f64);

    println!(
        "REPLAY: {total_events} events in {replay_secs:.3}s => {replay_throughput:.0} ev/s (disk read: {:.3}s, decode avg: {decode_avg_us:.2}µs/ev, apply avg: {apply_avg_us:.2}µs/ev)",
        read_all_duration.as_secs_f64()
    );

    let projection_digest = compute_projection_digest(&projection);
    assert_eq!(projection.repositories.len(), distinct_repos);

    let mut cold_reader = adapter.open_read().expect("cold open_read");
    let cold_envelopes = cold_reader.read_all_envelopes().expect("cold read");
    assert_eq!(cold_envelopes.len(), total_events);
    assert_eq!(
        cold_reader.rolling_commitment().current_commitment(),
        reader.rolling_commitment().current_commitment()
    );

    let mut cold_projection = EvidenceProjection::default();
    for env in &cold_envelopes {
        let domain_event = DomainEvent::decode_payload(&env.payload).expect("decode cold event");
        apply_event_to_projection(&mut cold_projection, &domain_event, env.header.detached);
    }
    let cold_digest = compute_projection_digest(&cold_projection);
    assert_eq!(
        projection_digest, cold_digest,
        "Cold projection digest must match warm projection digest"
    );
}

#[test]
fn test_staged_projection_benchmark_1k_10k() {
    run_staged_benchmark(1_000, 250, 20);
    run_staged_benchmark(10_000, 1_000, 50);
    if std::env::var("BENCH_SCALE").is_ok() {
        run_staged_benchmark(100_000, 2_500, 100);
    }
}

#[test]
fn test_staged_projection_benchmark_1m() {
    if std::env::var("BENCH_1M").is_err() {
        eprintln!("Skipping 1M benchmark; run with BENCH_1M=1 to execute.");
        return;
    }
    run_staged_benchmark(1_000_000, 5_000, 200);
}

fn expected_latest_seq(total_events: usize, distinct_repos: usize, repo_idx: usize) -> usize {
    let mut seq = repo_idx;
    let mut last = repo_idx;
    while seq < total_events {
        last = seq;
        seq += distinct_repos;
    }
    last
}

fn assert_expected_latest_state(
    projection: &EvidenceProjection,
    total_events: usize,
    distinct_repos: usize,
) {
    let snapshot = projection.sorted_snapshot();
    assert_eq!(snapshot.len(), distinct_repos, "fiber cardinality");
    let mut by_name = std::collections::HashMap::with_capacity(distinct_repos);
    for item in &snapshot {
        by_name.insert(item.repository.name.clone(), item);
    }
    for repo_idx in 0..distinct_repos {
        let seq = expected_latest_seq(total_events, distinct_repos, repo_idx);
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
            format!("{:?}", actual.checks.security_policy.status),
            format!("{:?}", expected.checks.security_policy.status),
            "{name} security_policy status at seq {seq}"
        );
        assert_eq!(
            format!("{:?}", actual.checks.branch_protection.status),
            format!("{:?}", expected.checks.branch_protection.status),
            "{name} branch_protection status at seq {seq}"
        );
        assert_eq!(
            actual.checks.security_policy.timestamp, expected.checks.security_policy.timestamp,
            "{name} latest timestamp (staleness/order oracle) at seq {seq}"
        );
    }
}

fn expected_event_nanos(seq: usize) -> u64 {
    (1_726_130_000 + seq as u64) * 1_000_000_000
}

fn assert_stored_chain_and_head_identity(
    envelopes: &[EventEnvelope],
    total_events: usize,
    distinct_repos: usize,
) {
    let mut head_by_fiber: std::collections::HashMap<[u8; 16], ([u8; 16], usize)> =
        std::collections::HashMap::with_capacity(distinct_repos);
    for (idx, env) in envelopes.iter().enumerate() {
        let fiber_id = env.header.fiber_id;
        if let Some((prev_event_id, prev_idx)) = head_by_fiber.get(&fiber_id) {
            assert_eq!(
                env.header.precursor, *prev_event_id,
                "same-fiber stored chain order violated at stream index {idx} (previous index {prev_idx})"
            );
        }
        head_by_fiber.insert(fiber_id, (env.header.event_id, idx));
    }
    assert_eq!(
        head_by_fiber.len(),
        distinct_repos,
        "stored-stream fiber cardinality"
    );

    for repo_idx in 0..distinct_repos {
        let fiber_id = derive_fiber_id(&format!("org/repo-{repo_idx}"));
        let (_, head_idx) = head_by_fiber
            .get(&fiber_id)
            .unwrap_or_else(|| panic!("missing stored fiber for repo-{repo_idx}"));
        let expected_idx = expected_latest_seq(total_events, distinct_repos, repo_idx);
        assert_eq!(
            *head_idx, expected_idx,
            "repo-{repo_idx} stored fiber head position"
        );
        let head_event = DomainEvent::decode_payload(&envelopes[*head_idx].payload)
            .expect("decode fiber head payload");
        match head_event {
            DomainEvent::RepositoryStateCaptured { timestamp, .. } => {
                assert_eq!(
                    timestamp.as_nanos(),
                    expected_event_nanos(expected_idx),
                    "repo-{repo_idx} fiber head identity (alias-free per-event marker)"
                );
            }
            other => panic!("unexpected fiber head event: {other:?}"),
        }
    }
}

#[test]
fn stale_by_3600_alias_witness() {
    let latest = generate_evidence(0, 9_950);
    let stale = generate_evidence(0, 6_350);
    assert_eq!(
        latest.checks.security_policy.timestamp, stale.checks.security_policy.timestamp,
        "generate_evidence aliases every 3600 sequences; evidence-equality is not a head oracle"
    );
    assert_eq!(
        format!("{:?}", latest.checks.security_policy.status),
        format!("{:?}", stale.checks.security_policy.status)
    );
    assert_ne!(
        expected_event_nanos(9_950),
        expected_event_nanos(6_350),
        "per-event envelope identity marker must distinguish the stale-by-3600 witness"
    );
}

fn write_fixture_artefact(
    adapter: &FileStorageAdapter,
    total_events: usize,
    distinct_repos: usize,
) -> (usize, std::time::Duration, std::time::Duration) {
    use std::io::Write as _;

    let mut meta_bytes = Vec::new();
    meta_bytes.extend_from_slice(&ContainerHeader::new().to_bytes());
    let mut claim_bytes = Vec::new();
    OwnershipRecord::OwnershipClaim(sample_claim(1)).encode(&mut claim_bytes);
    let mut claim_frame = Vec::new();
    ContainerFrame::encode_payload(&claim_bytes, &mut claim_frame);
    meta_bytes.extend_from_slice(&claim_frame);
    let mut meta_file = std::fs::File::create(adapter.meta_path()).expect("create .meta");
    meta_file.write_all(&meta_bytes).expect("write .meta");
    meta_file.sync_data().expect("sync .meta");

    let pgno_file = std::fs::File::create(adapter.pgno_path()).expect("create .pgno");
    let mut out = std::io::BufWriter::with_capacity(1 << 20, pgno_file);
    out.write_all(&ContainerHeader::new().to_bytes())
        .expect("write container header");

    let mut latest_env_by_fiber: std::collections::HashMap<[u8; 16], EventEnvelope> =
        std::collections::HashMap::with_capacity(distinct_repos);
    let mut bytes_written = 0usize;
    let mut gen_elapsed = std::time::Duration::ZERO;
    let mut io_elapsed = std::time::Duration::ZERO;

    for i in 0..total_events {
        let t_gen = Instant::now();
        let repo_idx = i % distinct_repos;
        let domain_key = format!("org/repo-{repo_idx}");
        let fiber_id = derive_fiber_id(&domain_key);
        let evidence = generate_evidence(repo_idx, i);
        let native_evidence: gh_report::event::RepositoryEvidence =
            evidence.try_into().expect("try_into native evidence");
        let domain_event = DomainEvent::RepositoryStateCaptured {
            domain_key: NonEmptyEventString::new(domain_key).unwrap(),
            repo_name: NonEmptyEventString::new(format!("repo-{repo_idx}")).unwrap(),
            timestamp: Timestamp::new((1_726_130_000 + i as u64) * 1_000_000_000).unwrap(),
            evidence: Some(native_evidence),
        };
        let mut payload = Vec::new();
        domain_event
            .encode_payload(&mut payload)
            .expect("encode payload");
        bytes_written += payload.len();

        let event_id = *uuid::Uuid::now_v7().as_bytes();
        let env = if let Some(prev) = latest_env_by_fiber.get(&fiber_id) {
            EventEnvelope::chain(prev, event_id, payload).expect("chain")
        } else {
            EventEnvelope::genesis(event_id, fiber_id, payload).expect("genesis")
        };
        let mut env_bytes = Vec::new();
        env.encode(&mut env_bytes);
        latest_env_by_fiber.insert(fiber_id, env);
        gen_elapsed += t_gen.elapsed();

        let t_io = Instant::now();
        let mut frame = Vec::new();
        ContainerFrame::encode_payload(&env_bytes, &mut frame);
        out.write_all(&frame).expect("write frame");
        io_elapsed += t_io.elapsed();
    }

    let t_io = Instant::now();
    let file = out.into_inner().expect("flush fixture");
    file.sync_data().expect("sync fixture");
    io_elapsed += t_io.elapsed();

    (bytes_written, gen_elapsed, io_elapsed)
}

fn run_fixture_projection_candidate(total_events: usize, distinct_repos: usize) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store_path = tmp.path().join(format!("fixture_{total_events}.pgno"));
    let adapter = FileStorageAdapter::new(&store_path);

    println!("\n--- [FIXTURE STAGE: {total_events} events, fibers: {distinct_repos}] ---");

    let start_build = Instant::now();
    let (bytes_written, gen_elapsed, io_elapsed) =
        write_fixture_artefact(&adapter, total_events, distinct_repos);
    let build_secs = start_build.elapsed().as_secs_f64();
    println!(
        "FIXTURE BUILD: {total_events} events in {build_secs:.3}s => {:.0} ev/s ({:.2} MB/s) [generate: {:.3}s, frame+io: {:.3}s]",
        (total_events as f64) / build_secs,
        ((bytes_written as f64) / (1024.0 * 1024.0)) / build_secs,
        gen_elapsed.as_secs_f64(),
        io_elapsed.as_secs_f64()
    );

    let start_replay = Instant::now();
    let mut reader = adapter.open_read().expect("open_read fixture");
    let envelopes = reader
        .read_all_envelopes()
        .expect("read_all_envelopes fixture");
    let read_all_duration = start_replay.elapsed();
    assert_eq!(envelopes.len(), total_events, "fixture event count");
    assert_stored_chain_and_head_identity(&envelopes, total_events, distinct_repos);

    let mut projection = EvidenceProjection::default();
    let start_apply = Instant::now();
    let mut decode_micros_total = 0u128;
    for env in &envelopes {
        let t0 = Instant::now();
        let domain_event = DomainEvent::decode_payload(&env.payload).expect("decode domain event");
        decode_micros_total += t0.elapsed().as_micros();
        apply_event_to_projection(&mut projection, &domain_event, env.header.detached);
    }
    let apply_duration = start_apply.elapsed();
    let replay_secs = start_replay.elapsed().as_secs_f64();
    println!(
        "FIXTURE REPLAY: {total_events} events in {replay_secs:.3}s => {:.0} ev/s (disk read: {:.3}s, nested decode avg: {:.2}\u{b5}s/ev, decode+apply avg: {:.2}\u{b5}s/ev)",
        (total_events as f64) / replay_secs,
        read_all_duration.as_secs_f64(),
        (decode_micros_total as f64) / (total_events as f64),
        (apply_duration.as_micros() as f64) / (total_events as f64)
    );

    let start_validate = Instant::now();
    let projection_digest = compute_projection_digest(&projection);
    assert_eq!(projection.repositories.len(), distinct_repos);
    assert_expected_latest_state(&projection, total_events, distinct_repos);

    let mut cold_reader = adapter.open_read().expect("cold open_read fixture");
    let cold_envelopes = cold_reader.read_all_envelopes().expect("cold read fixture");
    assert_eq!(cold_envelopes.len(), total_events);
    assert_eq!(
        cold_reader.rolling_commitment().current_commitment(),
        reader.rolling_commitment().current_commitment()
    );
    let mut cold_projection = EvidenceProjection::default();
    for env in &cold_envelopes {
        let domain_event = DomainEvent::decode_payload(&env.payload).expect("decode cold event");
        apply_event_to_projection(&mut cold_projection, &domain_event, env.header.detached);
    }
    assert_eq!(
        projection_digest,
        compute_projection_digest(&cold_projection),
        "Cold fixture projection digest must match warm fixture projection digest"
    );
    assert_expected_latest_state(&cold_projection, total_events, distinct_repos);
    println!(
        "FIXTURE VALIDATE: {:.3}s (expected-latest-state oracle over {distinct_repos} fibers)",
        start_validate.elapsed().as_secs_f64()
    );
}

#[test]
fn test_fixture_projection_candidate() {
    if std::env::var("BENCH_FIXTURE").is_ok() {
        run_fixture_projection_candidate(1_000, 20);
        run_fixture_projection_candidate(10_000, 50);
    } else {
        run_fixture_projection_candidate(200, 20);
    }
}
