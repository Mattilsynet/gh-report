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

#[repr(C)]
struct RUsage {
    ru_utime: [u64; 2],
    ru_stime: [u64; 2],
    ru_maxrss: i64,
    _pad: [i64; 13],
}

unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut RUsage) -> i32;
}

fn get_peak_rss_kb() -> u64 {
    let mut usage = std::mem::MaybeUninit::<RUsage>::zeroed();
    let res = unsafe { getrusage(0, usage.as_mut_ptr()) };
    if res == 0 {
        let usage = unsafe { usage.assume_init() };
        #[cfg(target_os = "macos")]
        {
            (usage.ru_maxrss as u64) / 1024
        }
        #[cfg(not(target_os = "macos"))]
        {
            usage.ru_maxrss as u64
        }
    } else {
        0
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
    let rss_initial_kb = get_peak_rss_kb();
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

        let verdict = session
            .append_batch_envelopes_detailed(&batch_envelopes)
            .expect("append batch");
        assert!(verdict.is_all_landed());
        events_written += chunk_len;
    }

    let write_duration = start_write.elapsed();
    let write_secs = write_duration.as_secs_f64();
    let write_throughput = (events_written as f64) / write_secs;
    let write_mb_s = ((bytes_written as f64) / (1024.0 * 1024.0)) / write_secs;

    println!(
        "WRITE: {events_written} events in {write_secs:.3}s => {write_throughput:.0} ev/s ({write_mb_s:.2} MB/s)"
    );
    assert!(
        write_secs < 60.0,
        "Write stage exceeded 60s target: {write_secs:.3}s"
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
    assert!(
        replay_secs < 60.0,
        "Replay stage exceeded 60s target: {replay_secs:.3}s"
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

    let rss_peak_kb = get_peak_rss_kb();
    println!(
        "RESOURCES: RSS initial: {} KiB, Peak: {} KiB, Δ: +{} KiB",
        rss_initial_kb,
        rss_peak_kb,
        rss_peak_kb.saturating_sub(rss_initial_kb)
    );
}

#[test]
fn test_staged_projection_benchmark_1k_10k_100k() {
    run_staged_benchmark(1_000, 250, 20);
    run_staged_benchmark(10_000, 1_000, 50);
    run_staged_benchmark(100_000, 2_500, 100);
}

#[test]
fn test_staged_projection_benchmark_1m() {
    if std::env::var("BENCH_1M").is_err() {
        eprintln!("Skipping 1M benchmark; run with BENCH_1M=1 to execute.");
        return;
    }
    run_staged_benchmark(1_000_000, 5_000, 200);
}
