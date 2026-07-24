use super::super::test_support::summary_with_files;
use super::transport::EnvVarSnapshot;
use crate::app::diagnostics::{
    agent_readiness_sidecar_runtime, build_summary_readiness, readiness_lane_output,
};
use crate::app::{build_agent_preflight_output, local_freshness_needs_refresh};
use crate::args::RetrievalStatusOutput;
use crate::runtime;
use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, ReadinessGoalDto, ReadinessStatusDto,
    StorageStatsDto,
};
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;

#[test]
fn local_freshness_refreshes_stale_and_not_checked_summaries() {
    let mut summary = summary_with_files(1);
    assert!(!local_freshness_needs_refresh(&summary));

    summary.freshness = Some(IndexFreshnessDto {
        status: IndexFreshnessStatusDto::Fresh,
        changed_file_count: 0,
        new_file_count: 0,
        removed_file_count: 0,
        checked_file_count: 1,
        indexed_file_count: 1,
        duration_ms: 1,
        reason: None,
        samples: Vec::new(),
    });
    assert!(!local_freshness_needs_refresh(&summary));

    summary.freshness.as_mut().expect("freshness").status = IndexFreshnessStatusDto::Stale;
    assert!(local_freshness_needs_refresh(&summary));

    summary.freshness.as_mut().expect("freshness").status = IndexFreshnessStatusDto::NotChecked;
    assert!(local_freshness_needs_refresh(&summary));
}

#[test]
fn agent_readiness_runtime_does_not_collapse_to_local_without_agent_run() {
    let _env_lock = crate::config::config_env_test_lock();
    let _env_snapshot = EnvVarSnapshot::clear(&[
        "CODESTORY_RETRIEVAL_PROFILE",
        "CODESTORY_RETRIEVAL_RUN_ID",
        "CI",
        "GITHUB_ACTIONS",
    ]);
    let temp = tempdir().expect("temp dir");
    let project = temp.path().join("repo");
    fs::create_dir_all(&project).expect("create project");

    let runtime = agent_readiness_sidecar_runtime(&project, None);

    assert_eq!(runtime.profile, codestory_retrieval::SidecarProfile::Agent);
    assert_eq!(
        runtime.run_id.as_deref(),
        Some(codestory_retrieval::DEFAULT_AGENT_RUN_ID)
    );
}

#[test]
fn readiness_lane_prefers_live_agent_status_over_aggregate_failure() {
    let sidecar = RetrievalStatusOutput {
        profile: Some("agent".to_string()),
        run_id: Some("run".to_string()),
        retrieval_mode: "full".to_string(),
        degraded_reason: None,
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "accelerated".to_string(),
        embedding_device_observation_source: "manual_env".to_string(),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: false,
        embedding_accelerator_request_provider: None,
        embedding_accelerator_request_device: None,
        embedding_cpu_allowed: false,
        manifest_generation: Some("generation".to_string()),
        manifest_input_hash: Some("hash".to_string()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    };
    let aggregate_verdict = codestory_contracts::api::ReadinessVerdictDto {
            goal: ReadinessGoalDto::AgentPacketSearch,
            status: ReadinessStatusDto::RepairRetrieval,
            summary: "retrieval is unavailable".to_string(),
            minimum_next: vec![
                "codestory-cli retrieval index --project C:/repo --profile agent --refresh auto --format json"
                    .to_string(),
            ],
            full_repair: Vec::new(),
            setup: None,
            index: None,
            sidecar: None,
        };

    let lane = readiness_lane_output(
        "agent_packet_search",
        &sidecar,
        Some(&aggregate_verdict),
        "C:/repo",
    );

    assert_eq!(lane.status, ReadinessStatusDto::Ready);
    assert_eq!(lane.retrieval_mode, "full");
    assert_eq!(lane.profile, "agent");
    assert_eq!(lane.run_id.as_deref(), Some("run"));
    assert!(
        lane.next_command
            .as_deref()
            .is_some_and(|command| command.contains("retrieval status")
                && command.contains("--profile agent")
                && command.contains("--run-id")
                && command.contains("--format json")),
        "ready agent lane should point at lane-scoped status proof: {lane:?}"
    );
}

#[test]
fn agent_preflight_allows_full_surfaces_from_full_agent_lane() {
    let local_default = RetrievalStatusOutput {
        profile: Some("local".to_string()),
        run_id: None,
        retrieval_mode: "unavailable".to_string(),
        degraded_reason: Some("retrieval_manifest_missing".to_string()),
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "unknown".to_string(),
        embedding_device_observation_source: "retrieval_unobserved".to_string(),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: false,
        embedding_accelerator_request_provider: None,
        embedding_accelerator_request_device: None,
        embedding_cpu_allowed: false,
        manifest_generation: None,
        manifest_input_hash: None,
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    };
    let agent_status = RetrievalStatusOutput {
        profile: Some("agent".to_string()),
        run_id: Some("run".to_string()),
        retrieval_mode: "full".to_string(),
        degraded_reason: None,
        embedding_device_policy: "cpu_allowed".to_string(),
        embedding_device_state: "cpu".to_string(),
        embedding_device_observation_source: "cpu_policy".to_string(),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: false,
        embedding_accelerator_request_provider: None,
        embedding_accelerator_request_device: None,
        embedding_cpu_allowed: true,
        manifest_generation: Some("generation".to_string()),
        manifest_input_hash: Some("hash".to_string()),
        precise_semantic_import_status: None,
        precise_semantic_import_reason: None,
        precise_semantic_import_revision: None,
        precise_semantic_import_producer: None,
    };
    let stats = StorageStatsDto {
        node_count: 1,
        edge_count: 0,
        file_count: 1,
        error_count: 0,
        fatal_error_count: 0,
    };
    let verdicts = build_summary_readiness("C:/repo", &stats, None, &agent_status);
    let agent_verdict = verdicts
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch);
    let mut readiness_lanes = BTreeMap::new();
    readiness_lanes.insert(
        "local_default".to_string(),
        readiness_lane_output("local_default", &local_default, None, "C:/repo"),
    );
    readiness_lanes.insert(
        "agent_packet_search".to_string(),
        readiness_lane_output(
            "agent_packet_search",
            &agent_status,
            agent_verdict,
            "C:/repo",
        ),
    );

    let output = build_agent_preflight_output(&verdicts, readiness_lanes, None);

    assert!(output.usable);
    assert_eq!(output.mode, "full_retrieval");
    assert_eq!(output.full_retrieval.status, ReadinessStatusDto::Ready);
    assert_eq!(
        output.full_retrieval.embedding_device_policy.as_deref(),
        Some("cpu_allowed")
    );
    assert_eq!(
        output.full_retrieval.embedding_device_state.as_deref(),
        Some("cpu")
    );
    assert_eq!(
        output
            .full_retrieval
            .embedding_device_observation_source
            .as_deref(),
        Some("cpu_policy")
    );
    assert_eq!(output.full_retrieval.embedding_cpu_allowed, Some(true));
    assert_eq!(
        output.local_default.status,
        ReadinessStatusDto::RepairRetrieval
    );
    assert!(
        output
            .local_default
            .next_command
            .as_deref()
            .is_some_and(|command| command.contains("--profile local")),
        "local/default blocker should name its lane-scoped next action: {output:#?}"
    );
    for surface in ["packet_full", "search_full", "context_full"] {
        assert!(
            output
                .safe_surfaces
                .iter()
                .any(|candidate| candidate == surface),
            "{surface} should be safe from the agent readiness lane: {output:#?}"
        );
        assert!(
            !output
                .blocked_surfaces
                .iter()
                .any(|candidate| candidate == surface),
            "{surface} should not be blocked by local/default retrieval: {output:#?}"
        );
    }
    assert!(
        output.next_command.is_none(),
        "ready local graph plus ready agent retrieval should not emit an aggregate next command: {output:#?}"
    );
}
