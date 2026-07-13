use super::machine_lock::*;
use super::native_lease::*;
use super::paths::*;
use super::scope::*;
use super::snapshot::*;
use super::types::*;
use super::*;
use crate::ready_repair_status;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

fn unique_resource(prefix: &str) -> String {
    format!("{prefix}-{}-{}", std::process::id(), now_epoch_ms())
}

fn cleanup_machine_resource(resource: &str) {
    let _ = fs::remove_file(machine_resource_lock_path(resource));
    let _ = fs::remove_file(machine_resource_reaper_lock_path(resource));
    let _ = fs::remove_file(machine_resource_reaper_takeover_lock_path(resource));
}

fn run_git(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(project)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_git_project(project: &Path, remote: &str) {
    run_git(project, &["init", "--quiet"]);
    run_git(
        project,
        &["config", "user.email", "codestory@example.invalid"],
    );
    run_git(project, &["config", "user.name", "CodeStory Test"]);
    run_git(
        project,
        &["commit", "--allow-empty", "--quiet", "-m", "initial"],
    );
    run_git(project, &["remote", "add", "origin", remote]);
}

fn test_scope(project: &Path, run_id: &str) -> BrokerScope {
    agent_repair_scope(project, Some(run_id), "9.9.9")
}

fn test_sidecar_runtime(
    project: &Path,
    profile: codestory_retrieval::SidecarProfile,
    run_id: Option<&str>,
) -> codestory_retrieval::SidecarRuntimeConfig {
    crate::sidecar_runtime::for_project_with_run_id_in_cache(
        Some(project),
        profile,
        run_id,
        &broker_cache_root(),
    )
}

fn write_machine_lock(resource: &str, scope: &BrokerScope, pid: u32) -> PathBuf {
    write_machine_lock_at(resource, scope, pid, now_epoch_ms())
}

fn write_machine_lock_at(
    resource: &str,
    scope: &BrokerScope,
    pid: u32,
    started_at_epoch_ms: i64,
) -> PathBuf {
    let path = machine_resource_lock_path(resource);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create lock parent");
    }
    let lock = BrokerMachineResourceLockFile {
        schema_version: MACHINE_LOCK_SCHEMA_VERSION,
        resource: resource.to_string(),
        scope: scope.clone(),
        pid,
        started_at_epoch_ms,
        process_start_identity: ready_repair_status::recorded_process_start_identity(pid),
        token: format!("test:{pid}:{started_at_epoch_ms}"),
        operation_id: broker_operation_id(scope),
        native_embedding_launch: None,
        native_embedding_quarantine_reason: None,
    };
    fs::write(
        &path,
        serde_json::to_vec_pretty(&lock).expect("serialize lock"),
    )
    .expect("write lock");
    path
}

fn write_stale_reaper_lock(resource: &str) -> PathBuf {
    let path = machine_resource_reaper_lock_path(resource);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create reaper lock parent");
    }
    let started_at_epoch_ms =
        now_epoch_ms() - MACHINE_REAPER_LOCK_STALE_TTL.as_millis() as i64 - 10_000;
    let lock = BrokerMachineResourceReaperLockFile {
        schema_version: MACHINE_LOCK_SCHEMA_VERSION,
        resource: resource.to_string(),
        pid: u32::MAX,
        started_at_epoch_ms,
        token: format!("stale-reaper:{started_at_epoch_ms}"),
    };
    fs::write(
        &path,
        serde_json::to_vec_pretty(&lock).expect("serialize reaper lock"),
    )
    .expect("write reaper lock");
    path
}

fn sample_snapshot(project: &Path) -> ReadinessBrokerSnapshot {
    let identity = codestory_workspace::project_identity_v3(project);
    let canonical_root_hash = identity.workspace_id.clone();
    ReadinessBrokerSnapshot {
        schema_version: BROKER_SCHEMA_VERSION,
        identity: Some(identity.clone()),
        install_id: "test-install".to_string(),
        project_id: identity.project_id,
        canonical_root_hash,
        workspace_root: clean_path(project),
        cli_version: "9.9.9".to_string(),
        updated_at_epoch_ms: now_epoch_ms(),
        snapshot_path: None,
        persistence_status: "pending".to_string(),
        persistence_error: None,
        operations: Vec::new(),
        resources: BTreeMap::new(),
        reconciliation: BrokerReconciliationSnapshot {
            status: "observed".to_string(),
            cleanup_performed: false,
            stale_status_paths_removed: Vec::new(),
            stale_lock_paths_removed: Vec::new(),
            abandoned_repairs: Vec::new(),
            local_refresh_cleanups: Vec::new(),
            active_repair: None,
            unresolved_orphan_reason: None,
        },
        gpu_proof: None,
    }
}

fn native_sidecar_state(spawned_at_epoch_ms: Option<i64>) -> codestory_retrieval::SidecarStateFile {
    codestory_retrieval::SidecarStateFile {
        project_identity: None,
        owner: "codestory".to_string(),
        profile: "agent".to_string(),
        namespace: "codestory-test".to_string(),
        compose_project: "codestory-test".to_string(),
        run_id: Some("shared-agent".to_string()),
        qdrant_http_port: 37032,
        qdrant_grpc_port: 37033,
        embed_http_port: 37040,
        embed_url: "http://127.0.0.1:37040/v1/embeddings".to_string(),
        embedding_endpoint_origin: Some(
            codestory_retrieval::EmbeddingEndpointOrigin::ManagedSidecar,
        ),
        embedding_endpoint_fingerprint_sha256: Some("hmac-sha256:fixture".to_string()),
        embedding_device_policy: "accelerator_required".to_string(),
        embedding_device_state: "gpu_verified".to_string(),
        embedding_device_observation_source: "test".to_string(),
        embedding_detected_provider: Some("vulkan".to_string()),
        embedding_detected_gpu: Some("Vulkan0".to_string()),
        embedding_accelerator_requested: true,
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: false,
        embedding_launch: Some(codestory_retrieval::EmbeddingLaunchMetadata {
            provider: "llamacpp".to_string(),
            launch_mode: codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
                .as_str()
                .to_string(),
            endpoint: "http://127.0.0.1:37040/v1/embeddings".to_string(),
            pid: Some(1234),
            spawned_at_epoch_ms,
            process_start_identity: Some("test-start-identity".to_string()),
            spawn_protocol: None,
            launch_args: vec!["--port".to_string(), "37040".to_string()],
            launch_fingerprint_sha256: Some("fingerprint".to_string()),
            executable_source: Some("test".to_string()),
            executable_path: Some("C:/cache/llama-server.exe".to_string()),
            model_path: Some("C:/cache/bge-base-en-v1.5.Q8_0.gguf".to_string()),
            log_path: Some("C:/cache/llama-server-native.log".to_string()),
            requested_device: Some("Vulkan0".to_string()),
        }),
        embedding_launch_ownership: codestory_retrieval::EmbeddingLaunchOwnership::Owner,
        sidecar_images: codestory_retrieval::default_sidecar_image_pins(),
        lexical_data_dir: "C:/cache/lexical".to_string(),
        qdrant_data_dir: "C:/cache/qdrant".to_string(),
        scip_artifacts_root: "C:/cache/scip".to_string(),
        compose_file: None,
        compose_started_by_bootstrap: true,
        cleanup_command: "codestory-cli retrieval down".to_string(),
        started_at_epoch_ms: 100,
    }
}

#[cfg(target_os = "macos")]
fn native_launch_for_pid(pid: u32) -> codestory_retrieval::EmbeddingLaunchMetadata {
    let mut launch = native_sidecar_state(Some(now_epoch_ms()))
        .embedding_launch
        .expect("native launch");
    launch.pid = Some(pid);
    launch
}

#[cfg(target_os = "macos")]
fn spawn_macos_native_process_fixture(
    endpoint: &str,
    port: u16,
) -> (
    std::process::Child,
    codestory_retrieval::EmbeddingLaunchMetadata,
) {
    let launch_args = vec![
        "-c".to_string(),
        "while :; do sleep 30; done".to_string(),
        "codestory-native".to_string(),
        "--port".to_string(),
        port.to_string(),
    ];
    let spawned_at_epoch_ms = now_epoch_ms();
    let child = Command::new("/bin/bash")
        .args(&launch_args)
        .spawn()
        .expect("spawn native-process fixture");
    let mut launch = native_launch_for_pid(child.id());
    launch.endpoint = endpoint.to_string();
    launch.spawned_at_epoch_ms = Some(spawned_at_epoch_ms);
    launch.process_start_identity =
        codestory_retrieval::native_embedding_process_start_identity(child.id())
            .expect("query native-process fixture start identity");
    launch.executable_path = Some("/bin/bash".to_string());
    launch.launch_args = launch_args;
    launch.launch_fingerprint_sha256 = Some("shell-fixture".to_string());
    launch.model_path = None;
    launch.log_path = None;
    (child, launch)
}

fn verified_gpu_proof_input(embed_smoke_ok: Option<bool>) -> BrokerGpuProofInput {
    BrokerGpuProofInput {
        embedding_device_policy: Some("accelerator_required".to_string()),
        embedding_device_state: Some("accelerated".to_string()),
        embedding_device_observation_source: Some("sidecar_log".to_string()),
        embedding_detected_provider: Some("vulkan".to_string()),
        embedding_detected_gpu: Some("Vulkan0".to_string()),
        embedding_accelerator_requested: Some(true),
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: Some(false),
        embed_smoke_ok,
        embed_smoke_ms: embed_smoke_ok.map(|_| 12),
        degraded_reason: None,
    }
}

fn gpu_runtime_identity(project: &Path, started_at_epoch_ms: i64) -> BrokerGpuRuntimeIdentity {
    BrokerGpuRuntimeIdentity {
        workspace_id: codestory_workspace::workspace_id_v3_for_root(project),
        profile: "agent".to_string(),
        run_id: Some("shared-agent".to_string()),
        namespace: "codestory-test".to_string(),
        compose_project: "codestory-test".to_string(),
        embed_url: "http://127.0.0.1:37040/v1/embeddings".to_string(),
        embedding_endpoint_origin: codestory_retrieval::EmbeddingEndpointOrigin::ManagedSidecar,
        embedding_endpoint_fingerprint_sha256: "hmac-sha256:fixture".to_string(),
        started_at_epoch_ms,
        embedding_launch: None,
    }
}

fn write_matching_native_sidecar_state(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    pid: u32,
) {
    let state = matching_native_sidecar_state(sidecar, pid);
    if let Some(parent) = sidecar.layout.state_file.parent() {
        fs::create_dir_all(parent).expect("create state parent");
    }
    fs::write(
        &sidecar.layout.state_file,
        serde_json::to_vec_pretty(&state).expect("serialize state"),
    )
    .expect("write state");
}

fn matching_native_sidecar_state(
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
    pid: u32,
) -> codestory_retrieval::SidecarStateFile {
    let mut state = native_sidecar_state(Some(now_epoch_ms()));
    state.project_identity = sidecar.project_identity.clone();
    state.profile = sidecar.profile.as_str().to_string();
    state.namespace = sidecar.namespace.clone();
    state.compose_project = sidecar.compose_project.clone();
    state.run_id = sidecar.run_id.clone();
    state.qdrant_http_port = sidecar.layout.qdrant_http_port;
    state.qdrant_grpc_port = sidecar.layout.qdrant_grpc_port;
    state.embed_http_port = sidecar.ownership().ports.embed_http;
    state.embed_url = sidecar.embedding.endpoint.clone();
    state.embedding_endpoint_origin = Some(sidecar.embedding.endpoint_origin);
    state.embedding_endpoint_fingerprint_sha256 =
        Some(sidecar.ownership().embedding_endpoint_fingerprint_sha256);
    if let Some(launch) = state.embedding_launch.as_mut() {
        launch.endpoint = state.embed_url.clone();
        launch.pid = Some(pid);
        launch.launch_args = vec!["--port".to_string(), state.embed_http_port.to_string()];
    }
    state
}

fn attach_native_launch_to_machine_lock(
    path: &Path,
    sidecar: &codestory_retrieval::SidecarRuntimeConfig,
) {
    let state: codestory_retrieval::SidecarStateFile = serde_json::from_slice(
        &fs::read(&sidecar.layout.state_file).expect("read native sidecar state"),
    )
    .expect("parse native sidecar state");
    let mut lock = read_machine_resource_lock_file(path).expect("read machine lock");
    lock.native_embedding_launch = state.embedding_launch;
    fs::write(
        path,
        serde_json::to_vec_pretty(&lock).expect("serialize lock"),
    )
    .expect("attach native launch to machine lock");
}

#[test]
fn gpu_proof_requires_observed_acceleration_when_requested() {
    let proof = gpu_proof(BrokerGpuProofInput {
        embedding_device_policy: Some("accelerator_required".to_string()),
        embedding_device_state: Some("unknown".to_string()),
        embedding_device_observation_source: Some("native_device_list".to_string()),
        embedding_detected_provider: None,
        embedding_detected_gpu: None,
        embedding_accelerator_requested: Some(true),
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: Some(false),
        embed_smoke_ok: None,
        embed_smoke_ms: None,
        degraded_reason: Some("embedding_device_unverified".to_string()),
    });
    assert_eq!(proof.proof_status, "gpu_unverified");
    assert!(!proof.meaningful_accelerator_work_proven);
    assert_eq!(proof.degraded_reason.as_deref(), Some("gpu_unverified"));
}

#[test]
fn gpu_proof_requires_live_embed_smoke_for_verified() {
    let without_smoke = gpu_proof(BrokerGpuProofInput {
        embedding_device_policy: Some("accelerator_required".to_string()),
        embedding_device_state: Some("accelerated".to_string()),
        embedding_device_observation_source: Some("native_log".to_string()),
        embedding_detected_provider: Some("vulkan".to_string()),
        embedding_detected_gpu: Some("Vulkan0".to_string()),
        embedding_accelerator_requested: Some(true),
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: Some(false),
        embed_smoke_ok: None,
        embed_smoke_ms: None,
        degraded_reason: None,
    });
    assert_eq!(without_smoke.proof_status, "gpu_unverified");
    assert!(!without_smoke.meaningful_accelerator_work_proven);

    let with_smoke = gpu_proof(BrokerGpuProofInput {
        embedding_device_policy: Some("accelerator_required".to_string()),
        embedding_device_state: Some("accelerated".to_string()),
        embedding_device_observation_source: Some("native_log".to_string()),
        embedding_detected_provider: Some("vulkan".to_string()),
        embedding_detected_gpu: Some("Vulkan0".to_string()),
        embedding_accelerator_requested: Some(true),
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: Some(false),
        embed_smoke_ok: Some(true),
        embed_smoke_ms: Some(42),
        degraded_reason: None,
    });
    assert_eq!(with_smoke.proof_status, "verified");
    assert!(with_smoke.meaningful_accelerator_work_proven);
    assert_eq!(with_smoke.embed_smoke_ok, Some(true));
    assert_eq!(with_smoke.embed_smoke_ms, Some(42));

    let with_smoke_json = serde_json::to_value(&with_smoke).expect("serialize gpu proof");
    assert!(
        with_smoke_json.get("embed_smoke_ok").is_some(),
        "gpu_proof JSON shape must include embed_smoke_ok when set: {with_smoke_json}"
    );
    assert!(
        with_smoke_json.get("embed_smoke_ms").is_some(),
        "gpu_proof JSON shape must include embed_smoke_ms when set: {with_smoke_json}"
    );
}

#[test]
fn gpu_proof_does_not_verify_from_device_inventory() {
    let proof = gpu_proof(BrokerGpuProofInput {
        embedding_device_policy: Some("accelerator_required".to_string()),
        embedding_device_state: Some("accelerated".to_string()),
        embedding_device_observation_source: Some("native_device_list".to_string()),
        embedding_detected_provider: Some("vulkan".to_string()),
        embedding_detected_gpu: Some("Vulkan0".to_string()),
        embedding_accelerator_requested: Some(true),
        embedding_accelerator_request_provider: Some("vulkan".to_string()),
        embedding_accelerator_request_device: Some("Vulkan0".to_string()),
        embedding_cpu_allowed: Some(false),
        embed_smoke_ok: Some(true),
        embed_smoke_ms: Some(42),
        degraded_reason: None,
    });
    assert_eq!(proof.proof_status, "gpu_unverified");
    assert!(!proof.meaningful_accelerator_work_proven);
    assert_eq!(proof.degraded_reason.as_deref(), Some("gpu_unverified"));
}

#[test]
fn broker_scope_carries_project_and_run_identity() {
    let project = tempdir().expect("temp project");
    let scope = agent_repair_scope(project.path(), Some("agent-1"), "9.9.9");
    assert_eq!(scope.schema_version, BROKER_SCHEMA_VERSION);
    assert_eq!(scope.profile, "agent");
    assert_eq!(scope.run_id.as_deref(), Some("agent-1"));
    assert_eq!(scope.agent_id.as_deref(), Some("agent-1"));
    let identity = scope.identity.as_ref().expect("broker identity");
    assert_eq!(scope.project_id, identity.project_id);
    assert_eq!(
        identity.workspace_id,
        codestory_workspace::workspace_id_v3_for_root(project.path())
    );
    assert_eq!(scope.cli_version, "9.9.9");
}

#[test]
fn broker_scope_accepts_repository_stable_project_id_with_dirty_artifact_scope() {
    let project = tempdir().expect("project");
    init_git_project(project.path(), "https://example.com/CodeStory/Fixture.git");
    fs::write(project.path().join("dirty.txt"), b"dirty\n").expect("dirty worktree");

    let scope = test_scope(project.path(), "shared-agent");
    let identity = effective_scope_identity(&scope).expect("valid dirty repository identity");

    assert!(!identity.portable_reuse_eligible);
    assert_eq!(identity.project_id, scope.project_id);
    assert_eq!(identity.artifact_scope_id, identity.workspace_id);
    assert!(!broker_operation_id(&scope).contains("invalid-workspace"));

    let resource = unique_resource("dirty-v3-native-handoff");
    cleanup_machine_resource(&resource);
    let lock = match try_acquire_machine_resource_lock(&resource, &scope)
        .expect("acquire dirty repository machine lock")
    {
        BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
        BrokerMachineResourceLockAttempt::Busy(busy) => {
            panic!("dirty repository lock unexpectedly busy: {busy:?}")
        }
    };
    assert!(read_machine_resource_lock_file(&lock.path).is_some());
    let mut lease = Some(BrokerNativeEmbeddingResourceLease::Acquired(lock));
    let state = native_sidecar_state(Some(now_epoch_ms()));
    let pid = state
        .embedding_launch
        .as_ref()
        .and_then(|launch| launch.pid)
        .expect("native launch pid");
    transfer_native_embedding_resource_lease_with_validator(&mut lease, &state, |_| Ok(pid))
        .expect("handoff dirty repository machine lock");
    let handed_off = read_machine_resource_lock_file(&machine_resource_lock_path(&resource))
        .expect("read handed-off dirty repository lock");
    assert_eq!(handed_off.pid, pid);
    assert!(handed_off.native_embedding_launch.is_some());
    cleanup_machine_resource(&resource);
}

#[test]
fn broker_persistence_preserves_non_default_repository_ports() {
    let first = tempdir().expect("first project");
    let second = tempdir().expect("second project");
    init_git_project(
        first.path(),
        "ssh://git@example.com:2222/Org/CaseSensitive.git",
    );
    init_git_project(
        second.path(),
        "ssh://git@example.com:2200/Org/CaseSensitive.git",
    );

    let first = test_scope(first.path(), "shared-agent");
    let second = test_scope(second.path(), "shared-agent");
    assert_ne!(first.project_id, second.project_id);
    assert_ne!(
        first.identity.unwrap().canonical_repository_id,
        second.identity.unwrap().canonical_repository_id
    );
}

#[cfg(unix)]
#[test]
fn broker_scope_tracks_host_filesystem_case_identity() {
    let parent = tempdir().expect("parent");
    let upper = parent.path().join("Project");
    let lower = parent.path().join("project");
    fs::create_dir_all(&upper).expect("upper project");
    fs::create_dir_all(&lower).expect("lower project");

    let upper = test_scope(&upper, "shared-agent");
    let lower = test_scope(&lower, "shared-agent");
    if codestory_workspace::same_workspace_path(
        Path::new(&upper.workspace_root),
        Path::new(&lower.workspace_root),
    ) {
        assert_eq!(upper.canonical_root_hash, lower.canonical_root_hash);
        assert_eq!(broker_operation_id(&upper), broker_operation_id(&lower));
    } else {
        assert_ne!(upper.canonical_root_hash, lower.canonical_root_hash);
        assert_ne!(broker_operation_id(&upper), broker_operation_id(&lower));
    }
}

#[cfg(windows)]
#[test]
fn broker_scope_treats_windows_case_alias_as_one_workspace() {
    let project = tempdir().expect("project");
    let alias = PathBuf::from(project.path().to_string_lossy().to_ascii_uppercase());
    let original = test_scope(project.path(), "shared-agent");
    let alias = test_scope(&alias, "shared-agent");
    assert_eq!(original.canonical_root_hash, alias.canonical_root_hash);
    assert_eq!(broker_operation_id(&original), broker_operation_id(&alias));
}

#[test]
fn native_embedding_reuse_does_not_cross_workspace_roots() {
    let project = tempdir().expect("project");
    let other_worktree = tempdir().expect("other worktree");
    let scope = test_scope(project.path(), "shared-agent");
    let sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("shared-agent"),
    );
    let busy = BrokerMachineResourceBusy {
        snapshot: BrokerResourceSnapshot {
            resource: NATIVE_EMBEDDING_RESOURCE.to_string(),
            scope: "machine".to_string(),
            status: "busy".to_string(),
            owner_pid: Some(std::process::id()),
            owner_operation_id: Some("other-worktree".to_string()),
            owner_project_id: Some(scope.project_id.clone()),
            owner_workspace_root: Some(clean_path(other_worktree.path())),
            started_at_epoch_ms: Some(now_epoch_ms()),
            lock_path: "other-worktree.lock".to_string(),
            queued_reason: Some("machine_resource_busy".to_string()),
        },
    };
    let mut validate = |_launch: &codestory_retrieval::EmbeddingLaunchMetadata| {
        panic!("different workspace must not reach launch validation")
    };

    assert_eq!(
        reusable_native_embedding_resource_pid(&scope, &sidecar, &busy, &mut validate)
            .expect("workspace comparison"),
        None
    );
}

#[test]
fn native_embedding_busy_lock_reuses_matching_sidecar_owner() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("native-reuse");
    cleanup_machine_resource(&resource);
    let owner_scope = operation_scope(
        project.path(),
        "local",
        None,
        "retrieval_bootstrap",
        "9.9.9",
    );
    let requested_scope = test_scope(project.path(), "shared-agent");
    let owner_sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Local,
        None,
    );
    let requested_sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("shared-agent"),
    );
    let owner_pid = std::process::id();
    let lock_path = write_machine_lock(&resource, &owner_scope, owner_pid);
    write_matching_native_sidecar_state(&owner_sidecar, owner_pid);
    attach_native_launch_to_machine_lock(&lock_path, &owner_sidecar);
    let mut snapshot = machine_resource_snapshot(&resource);
    snapshot.status = "busy".to_string();
    let busy = BrokerMachineResourceBusy { snapshot };
    let validator_called = std::cell::Cell::new(false);

    let reused = reusable_native_embedding_resource_launch_with_matcher(
        &requested_scope,
        &requested_sidecar,
        &busy,
        &mut |launch| {
            validator_called.set(true);
            assert_eq!(launch.pid, Some(owner_pid));
            Ok(owner_pid)
        },
        |scope, retargeted, launch| {
            assert!(
                validator_called.get(),
                "process identity must be exact before runtime configuration matching"
            );
            assert_eq!(scope.operation_kind, "retrieval_bootstrap");
            assert_eq!(scope.profile, "local");
            assert_ne!(
                broker_operation_id(scope),
                broker_operation_id(&requested_scope)
            );
            assert_eq!(
                retargeted.profile,
                codestory_retrieval::SidecarProfile::Agent
            );
            assert_eq!(retargeted.run_id.as_deref(), Some("shared-agent"));
            assert_ne!(retargeted.embed_http_port, owner_sidecar.embed_http_port);
            assert_eq!(retargeted.embedding.endpoint, launch.endpoint);
            Ok(true)
        },
    )
    .expect("reuse check");

    assert_eq!(reused.and_then(|launch| launch.pid), Some(owner_pid));
    assert!(validator_called.get());
    cleanup_machine_resource(&resource);
}

#[test]
fn quarantined_native_embedding_launch_is_never_reusable() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("native-quarantine-no-reuse");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "shared-agent");
    let sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("shared-agent"),
    );
    let owner_pid = std::process::id();
    let lock_path = write_machine_lock(&resource, &scope, owner_pid);
    write_matching_native_sidecar_state(&sidecar, owner_pid);
    attach_native_launch_to_machine_lock(&lock_path, &sidecar);
    let mut lock = read_machine_resource_lock_file(&lock_path).expect("machine lock");
    lock.native_embedding_quarantine_reason = Some("finalization_pending".to_string());
    fs::write(
        &lock_path,
        serde_json::to_vec_pretty(&lock).expect("serialize quarantine"),
    )
    .expect("write quarantine");
    let mut snapshot = machine_resource_snapshot(&resource);
    snapshot.status = "busy".to_string();
    let busy = BrokerMachineResourceBusy { snapshot };

    let reused = reusable_native_embedding_resource_launch_with_matcher(
        &scope,
        &sidecar,
        &busy,
        &mut |_| panic!("quarantine must be rejected before identity validation"),
        |_, _, _| panic!("quarantine must be rejected before runtime matching"),
    )
    .expect("quarantine reuse check");

    assert!(reused.is_none());
    assert_eq!(
        busy.snapshot.queued_reason.as_deref(),
        Some("native_embedding_cleanup_pending")
    );
    cleanup_machine_resource(&resource);
}

#[test]
fn native_embedding_busy_lock_rejects_mismatched_state_pid() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("native-mismatch");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "shared-agent");
    let sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("shared-agent"),
    );
    let owner_pid = std::process::id();
    let lock_path = write_machine_lock(&resource, &scope, owner_pid);
    write_matching_native_sidecar_state(&sidecar, owner_pid.saturating_add(1));
    attach_native_launch_to_machine_lock(&lock_path, &sidecar);
    let busy = BrokerMachineResourceBusy {
        snapshot: machine_resource_snapshot(&resource),
    };

    let reused = reusable_native_embedding_resource_launch_with_matcher(
        &scope,
        &sidecar,
        &busy,
        &mut |_| panic!("mismatched pid must not reach live identity validation"),
        |_, _, _| panic!("mismatched pid must not reach runtime matching"),
    )
    .expect("reuse check");

    assert_eq!(reused, None);
    cleanup_machine_resource(&resource);
}

#[test]
fn warm_reused_agent_state_binds_exact_broker_runtime_identity() {
    let project = tempdir().expect("project");
    let run_id = "shared-agent";
    let mut initial = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
    );
    let allocated_embed_port = initial.embed_http_port;
    let shared_embed_port = if allocated_embed_port == 18080 {
        18081
    } else {
        18080
    };
    initial
        .use_broker_verified_native_embedding_endpoint(shared_embed_port)
        .expect("retarget verified native endpoint");
    let mut state = native_sidecar_state(Some(now_epoch_ms()));
    state.project_identity = initial.project_identity.clone();
    state.profile = initial.profile.as_str().to_string();
    state.namespace = initial.namespace.clone();
    state.compose_project = initial.compose_project.clone();
    state.run_id = initial.run_id.clone();
    state.qdrant_http_port = initial.layout.qdrant_http_port;
    state.qdrant_grpc_port = initial.layout.qdrant_grpc_port;
    state.embed_http_port = shared_embed_port;
    state.embed_url = initial.embedding.endpoint.clone();
    state.embedding_endpoint_origin = Some(initial.embedding.endpoint_origin);
    state.embedding_endpoint_fingerprint_sha256 =
        Some(initial.ownership().embedding_endpoint_fingerprint_sha256);
    state.started_at_epoch_ms = now_epoch_ms();
    if let Some(launch) = state.embedding_launch.as_mut() {
        launch.endpoint = state.embed_url.clone();
        launch.pid = Some(std::process::id());
    }
    fs::create_dir_all(initial.layout.state_file.parent().expect("state parent"))
        .expect("state dir");
    fs::write(
        &initial.layout.state_file,
        serde_json::to_vec_pretty(&state).expect("serialize native state"),
    )
    .expect("persist native state");

    let warm = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
    );
    assert_eq!(warm.embed_http_port, allocated_embed_port);
    assert_eq!(warm.ownership().ports.embed_http, shared_embed_port);
    assert_eq!(warm.embedding.endpoint, state.embed_url);

    // This seam tests broker identity binding independently from the Darwin
    // process probe; native PID/executable identity has focused coverage in
    // codestory-retrieval and is required when a launch is present.
    state.embedding_launch = None;
    fs::write(
        &warm.layout.state_file,
        serde_json::to_vec_pretty(&state).expect("serialize identity state"),
    )
    .expect("persist identity state");
    assert!(codestory_retrieval::sidecar_state_matches_runtime(
        &state, &warm
    ));
    let expected_identity = codestory_workspace::project_identity_v3(project.path());
    let identity = gpu_runtime_identity_for_sidecar(&warm, project.path(), &expected_identity)
        .expect("warm broker runtime identity");
    assert_eq!(identity.workspace_id, expected_identity.workspace_id);
    assert_eq!(identity.profile, "agent");
    assert_eq!(identity.run_id.as_deref(), Some(run_id));
    assert_eq!(identity.embed_url, state.embed_url);

    let mut external = warm.clone();
    external.embedding.endpoint =
        "https://embedding.example/v1/embeddings?token=secret".to_string();
    external.embedding.endpoint_origin =
        codestory_retrieval::EmbeddingEndpointOrigin::ProcessEnvironment;
    external.embedding.server_launch = Some("external_endpoint".to_string());
    assert!(
        gpu_runtime_identity_for_sidecar(&external, project.path(), &expected_identity).is_none(),
        "external endpoints must never supply managed broker GPU identity"
    );
}

#[test]
fn borrower_down_leaves_owner_native_process_and_machine_lock_intact() {
    let project = tempdir().expect("project");
    let resource = unique_resource("native-borrower-down");
    cleanup_machine_resource(&resource);
    let owner_scope = test_scope(project.path(), "owner-agent");
    let mut owner_runtime = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("owner-agent"),
    );
    let mut borrower_runtime = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("borrower-agent"),
    );
    let shared_pid = std::process::id();
    let mut owner_state = matching_native_sidecar_state(&owner_runtime, shared_pid);
    {
        let launch = owner_state.embedding_launch.as_mut().expect("owner launch");
        launch.pid = Some(shared_pid);
        launch.endpoint.clone_from(&owner_state.embed_url);
        launch.launch_args = vec![
            "--port".to_string(),
            owner_state.embed_http_port.to_string(),
        ];
    }
    owner_runtime
        .use_broker_verified_native_embedding_endpoint(owner_state.embed_http_port)
        .expect("owner endpoint");
    borrower_runtime
        .use_broker_verified_native_embedding_endpoint(owner_state.embed_http_port)
        .expect("borrower endpoint");

    let mut lock = match try_acquire_machine_resource_lock(&resource, &owner_scope)
        .expect("acquire owner machine lock")
    {
        BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
        BrokerMachineResourceLockAttempt::Busy(busy) => {
            panic!("owner should acquire machine lock, got {busy:?}")
        }
    };
    assert!(
        transfer_machine_resource_lock_to_native_launch(
            &mut lock,
            owner_state.embedding_launch.as_ref().expect("owner launch")
        )
        .expect("handoff owner lock")
    );
    let lock_path = machine_resource_lock_path(&resource);
    for (path, state) in [
        (&owner_runtime.layout.state_file, owner_state.clone()),
        (
            &borrower_runtime.layout.state_file,
            codestory_retrieval::SidecarStateFile {
                project_identity: borrower_runtime.project_identity.clone(),
                profile: borrower_runtime.profile.as_str().to_string(),
                namespace: borrower_runtime.namespace.clone(),
                compose_project: borrower_runtime.compose_project.clone(),
                run_id: borrower_runtime.run_id.clone(),
                qdrant_http_port: borrower_runtime.layout.qdrant_http_port,
                qdrant_grpc_port: borrower_runtime.layout.qdrant_grpc_port,
                embed_http_port: owner_state.embed_http_port,
                embed_url: owner_state.embed_url.clone(),
                embedding_launch_ownership: codestory_retrieval::EmbeddingLaunchOwnership::Attached,
                ..owner_state.clone()
            },
        ),
    ] {
        fs::create_dir_all(path.parent().expect("state parent")).expect("create state parent");
        fs::write(
            path,
            serde_json::to_vec_pretty(&state).expect("serialize state"),
        )
        .expect("write state");
    }

    let borrower_owned_launch = native_embedding_launch_from_sidecar_state_file(&borrower_runtime)
        .expect("read borrower state");
    codestory_retrieval::sidecar_down_for_runtime(&borrower_runtime)
        .expect("borrower down must not stop owner pid");
    if let Some(launch) = borrower_owned_launch.as_ref() {
        release_machine_resource_lock_for_native_launch(&resource, launch)
            .expect("release borrower-owned launch");
    }

    assert_eq!(
        std::process::id(),
        shared_pid,
        "shared owner pid remains live"
    );
    assert!(owner_runtime.layout.state_file.exists());
    assert!(!borrower_runtime.layout.state_file.exists());
    assert!(
        lock_path.exists(),
        "borrower must not release the owner lock"
    );
    cleanup_machine_resource(&resource);
}

#[test]
fn native_embedding_bind_retry_exhaustion_is_bounded_to_two_rotations() {
    let project = tempdir().expect("project");
    let resource = unique_resource("bind-retry-exhaustion");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "bind-retry-exhaustion");
    let mut sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("bind-retry-exhaustion"),
    );
    sidecar.embedding.server_launch = Some(
        codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
            .as_str()
            .to_string(),
    );
    let mut attempted_ports = Vec::new();
    let mut competitors = Vec::new();

    let error = run_with_native_embedding_lease_lifecycle(
        NativeEmbeddingLeaseLifecycleParams {
            scope: &scope,
            sidecar: &mut sidecar,
            resource: &resource,
            wait: Duration::ZERO,
            poll: Duration::ZERO,
            bootstrap_context: "exhaustion bootstrap",
            sidecar_cleanup_label: "exhaustion sidecar",
        },
        |selected, _, _, _| {
            let port = selected.ownership().ports.embed_http;
            attempted_ports.push(port);
            competitors.push(
                std::net::TcpListener::bind(("127.0.0.1", port))
                    .expect("hold simulated competing listener"),
            );
            anyhow::bail!(
                "{}: simulated persistent competitor",
                codestory_retrieval::NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON
            )
        },
        |state: &codestory_retrieval::SidecarStateFile| state,
        |_, _| -> anyhow::Result<()> { unreachable!("bootstrap must fail") },
    )
    .expect_err("persistent bind failures must exhaust the bounded retry budget");

    assert!(
        format!("{error:#}")
            .contains(codestory_retrieval::NATIVE_EMBEDDING_PORT_BIND_FAILED_REASON)
    );
    assert_eq!(attempted_ports.len(), 3, "initial attempt plus two retries");
    let distinct = attempted_ports
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), 3, "every retry must rotate the leased port");
    cleanup_machine_resource(&resource);
}

#[test]
fn state_write_and_stop_failure_keeps_exact_native_launch_quarantined() {
    let project = tempdir().expect("project");
    let resource = unique_resource("state-write-stop-failure");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "state-write-stop-failure");
    let mut sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("state-write-stop-failure"),
    );
    sidecar.embedding.server_launch = Some(
        codestory_retrieval::EmbeddingServerLaunchMode::NativeSpawned
            .as_str()
            .to_string(),
    );
    let child_pid = 4321;

    let error = run_with_native_embedding_lease_lifecycle(
        NativeEmbeddingLeaseLifecycleParams {
            scope: &scope,
            sidecar: &mut sidecar,
            resource: &resource,
            wait: Duration::ZERO,
            poll: Duration::ZERO,
            bootstrap_context: "state publication bootstrap",
            sidecar_cleanup_label: "state publication sidecar",
        },
        |selected, _, _, observe_new_native_launch| {
            let state = matching_native_sidecar_state(selected, child_pid);
            let launch = state.embedding_launch.as_ref().expect("native launch");
            observe_new_native_launch(launch)?;
            let cleanup_error = anyhow::anyhow!("stop_failed");
            Err::<codestory_retrieval::SidecarStateFile, _>(
                anyhow::anyhow!("state_write_failed").context(
                    codestory_retrieval::NativeEmbeddingStartupCleanupFailure::new(
                        launch.clone(),
                        &cleanup_error,
                    ),
                ),
            )
        },
        |state| state,
        |_, _| -> anyhow::Result<()> { unreachable!("bootstrap must fail") },
    )
    .expect_err("failed state publication and stop must retain quarantine");

    assert!(format!("{error:#}").contains("state_write_failed"));
    let lock = read_machine_resource_lock_file(&machine_resource_lock_path(&resource))
        .expect("durable quarantine");
    assert_eq!(lock.pid, std::process::id(), "launcher remains lock owner");
    assert_eq!(
        lock.native_embedding_launch
            .as_ref()
            .and_then(|launch| launch.pid),
        Some(child_pid)
    );
    assert!(lock.native_embedding_quarantine_reason.is_some());
    cleanup_machine_resource(&resource);
}

#[test]
fn machine_resource_lock_reports_busy_until_owner_drops() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("single-owner");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "owner");

    let lock =
        match try_acquire_machine_resource_lock(&resource, &scope).expect("acquire machine lock") {
            BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                panic!("first lock should acquire, got {busy:?}")
            }
        };
    let busy = match try_acquire_machine_resource_lock(&resource, &scope).expect("second acquire") {
        BrokerMachineResourceLockAttempt::Acquired(_) => {
            panic!("second lock should be busy")
        }
        BrokerMachineResourceLockAttempt::Busy(busy) => busy,
    };
    assert_eq!(busy.snapshot.status, "busy");
    assert_eq!(busy.snapshot.owner_pid, Some(std::process::id()));

    drop(lock);
    let reacquired =
        try_acquire_machine_resource_lock(&resource, &scope).expect("reacquire after drop");
    assert!(matches!(
        reacquired,
        BrokerMachineResourceLockAttempt::Acquired(_)
    ));
    cleanup_machine_resource(&resource);
}

#[test]
fn machine_resource_lock_reclaims_dead_owner() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("dead-owner");
    cleanup_machine_resource(&resource);
    let old_scope = test_scope(project.path(), "dead");
    let new_scope = test_scope(project.path(), "new");
    write_machine_lock(&resource, &old_scope, exited_process_id());

    let acquired =
        try_acquire_machine_resource_lock(&resource, &new_scope).expect("reclaim dead owner");
    assert!(matches!(
        acquired,
        BrokerMachineResourceLockAttempt::Acquired(_)
    ));
    let snapshot = machine_resource_snapshot(&resource);
    assert_eq!(snapshot.status, "busy");
    assert_eq!(
        snapshot.owner_operation_id,
        Some(broker_operation_id(&new_scope))
    );
    cleanup_machine_resource(&resource);
}

#[cfg(target_os = "macos")]
#[test]
fn aborted_pre_handoff_owner_exactly_stops_published_native_child_before_reacquire() {
    let project = tempdir().expect("project");
    let resource = unique_resource("aborted-pre-handoff-owner");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "aborted-owner");
    let sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some("aborted-owner"),
    );

    let mut launcher = Command::new("/bin/sh")
        .args(["-c", "exit 99"])
        .spawn()
        .expect("spawn launcher fixture");
    let launcher_pid = launcher.id();
    let status = launcher.wait().expect("wait abrupt launcher exit");
    assert_eq!(status.code(), Some(99));

    let (mut native_child, launch) = spawn_macos_native_process_fixture(
        &sidecar.embedding.endpoint,
        sidecar.ownership().ports.embed_http,
    );
    let mut state = matching_native_sidecar_state(&sidecar, native_child.id());
    state.embedding_launch = Some(launch);
    fs::create_dir_all(
        sidecar
            .layout
            .state_file
            .parent()
            .expect("sidecar state parent"),
    )
    .expect("create state parent");
    fs::write(
        &sidecar.layout.state_file,
        serde_json::to_vec_pretty(&state).expect("serialize owner state"),
    )
    .expect("publish owner state before abrupt exit");
    write_machine_lock(&resource, &scope, launcher_pid);

    let acquired = try_acquire_native_embedding_machine_resource_lock(&resource, &scope)
        .expect("recover stale pre-handoff owner");

    assert!(
        matches!(acquired, BrokerMachineResourceLockAttempt::Acquired(_)),
        "recovery must clean the exact old child before granting a new launcher"
    );
    assert!(
        !sidecar.layout.state_file.exists(),
        "stale owner state must be removed only after exact child cleanup"
    );
    let child_status = native_child
        .wait()
        .expect("reap native-child fixture after exact cleanup");
    assert!(
        !child_status.success(),
        "the surviving native child must be terminated before reacquire"
    );
    drop(acquired);
    cleanup_machine_resource(&resource);
}

#[test]
fn machine_resource_cache_fingerprint_tracks_lock_and_reaper_changes() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("cache-fingerprint");
    cleanup_machine_resource(&resource);
    let before = machine_resource_cache_fingerprint(&resource);
    let scope = test_scope(project.path(), "owner");

    write_machine_lock(&resource, &scope, std::process::id());
    let after_lock = machine_resource_cache_fingerprint(&resource);
    assert_ne!(before, after_lock);

    write_stale_reaper_lock(&resource);
    let after_reaper = machine_resource_cache_fingerprint(&resource);
    assert_ne!(after_lock, after_reaper);
    cleanup_machine_resource(&resource);
}

#[test]
fn path_fingerprint_tracks_same_length_content_changes() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("lock.json");
    fs::write(&path, b"aaaa").expect("write first");
    let first = path_fingerprint(&path);

    fs::write(&path, b"bbbb").expect("write second");
    let second = path_fingerprint(&path);

    assert_ne!(first, second);
}

#[test]
fn native_launch_handoff_requires_exact_process_start_identity() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("pid-transfer-start-identity");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "owner");
    let mut lock =
        match try_acquire_machine_resource_lock(&resource, &scope).expect("acquire machine lock") {
            BrokerMachineResourceLockAttempt::Acquired(lock) => lock,
            BrokerMachineResourceLockAttempt::Busy(busy) => {
                panic!("first lock should acquire, got {busy:?}")
            }
        };
    let mut state = native_sidecar_state(Some(now_epoch_ms()));
    let launch = state.embedding_launch.as_mut().expect("launch");
    launch.pid = Some(std::process::id());
    launch.process_start_identity = None;

    let error = transfer_machine_resource_lock_to_native_launch(&mut lock, launch)
        .expect_err("final handoff without exact start identity must fail closed");

    assert!(format!("{error:#}").contains("missing exact process start identity"));
    assert!(lock.release_on_drop);
    drop(lock);
    cleanup_machine_resource(&resource);
}

#[test]
fn snapshot_file_round_trips_json() {
    let dir = tempdir().expect("temp dir");
    let snapshot = sample_snapshot(dir.path());
    let path = dir.path().join("snapshot.json");

    write_snapshot_file(&path, &snapshot).expect("write snapshot");

    let parsed: ReadinessBrokerSnapshot =
        serde_json::from_str(&fs::read_to_string(&path).expect("read snapshot"))
            .expect("parse snapshot");
    assert_eq!(parsed.schema_version, BROKER_SCHEMA_VERSION);
    assert_eq!(parsed.project_id, snapshot.project_id);
}

#[test]
fn v2_snapshot_identity_maps_only_to_its_proven_workspace() {
    let project = tempdir().expect("project");
    let other = tempdir().expect("other project");
    let legacy_identity = codestory_workspace::project_identity_v2(project.path());
    let legacy_hash = hash_text(&clean_path_text(project.path()));
    let mut value = serde_json::to_value(sample_snapshot(project.path())).expect("snapshot json");
    value["schema_version"] = BROKER_SCHEMA_VERSION_V2.into();
    value["identity"] = serde_json::to_value(&legacy_identity).expect("legacy identity json");
    value["project_id"] = legacy_identity.project_id.clone().into();
    value["canonical_root_hash"] = legacy_hash.into();

    let parsed: ReadinessBrokerSnapshot =
        serde_json::from_value(value.clone()).expect("parse v2 snapshot");
    let effective = parsed.effective_identity().expect("map v2 identity");
    assert_eq!(
        effective.workspace_id,
        codestory_workspace::workspace_id_v3_for_root(project.path())
    );
    assert_eq!(
        effective.project_identity_schema_version,
        codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
    );

    value["workspace_root"] = clean_path(other.path()).into();
    let mismatched: ReadinessBrokerSnapshot =
        serde_json::from_value(value).expect("parse mismatched v2 snapshot");
    assert!(mismatched.effective_identity().is_none());
}

#[test]
fn snapshot_file_uses_unique_temp_names_for_same_process_writers() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("snapshot.json");
    let snapshot = sample_snapshot(dir.path());
    let mut handles = Vec::new();

    for index in 0..4 {
        let path = path.clone();
        let mut snapshot = snapshot.clone();
        snapshot.cli_version = format!("9.9.{index}");
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                write_snapshot_file(&path, &snapshot).expect("write snapshot");
            }
        }));
    }

    for handle in handles {
        handle.join().expect("snapshot writer thread");
    }
    let parsed: ReadinessBrokerSnapshot =
        serde_json::from_str(&fs::read_to_string(&path).expect("read final snapshot"))
            .expect("parse final snapshot");
    assert_eq!(parsed.schema_version, BROKER_SCHEMA_VERSION);
    assert!(parsed.cli_version.starts_with("9.9."));
}

fn write_ready_repair_status_file(
    project: &Path,
    run_id: &str,
    pid: u32,
    updated_at_epoch_ms: i64,
    phase: &str,
) -> PathBuf {
    let sidecar = test_sidecar_runtime(
        project,
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
    );
    let path = sidecar
        .layout
        .state_file
        .with_file_name("ready-repair-status.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create repair status parent");
    }
    let status = serde_json::json!({
        "schema_version": 1,
        "status": "repairing",
        "project_root": producer_path_text(project),
        "profile": "agent",
        "run_id": run_id,
        "namespace": sidecar.namespace,
        "compose_project": sidecar.compose_project,
        "phase": phase,
        "pid": pid,
        "started_at_epoch_ms": updated_at_epoch_ms,
        "updated_at_epoch_ms": updated_at_epoch_ms,
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&status).expect("status json"),
    )
    .expect("write repair status");
    path
}

fn producer_path_text(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

fn exited_process_id() -> u32 {
    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "exit", "0"])
        .spawn()
        .expect("spawn short-lived process");
    #[cfg(not(windows))]
    let mut child = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .expect("spawn short-lived process");
    let pid = child.id();
    child.wait().expect("wait for short-lived process");
    pid
}

fn write_stale_local_refresh(cache_root: &Path, project: &Path) {
    let status_path = cache_root.join("local-refresh-status.json");
    let lock_path = cache_root.join("local-refresh.lock");
    let old_started = now_epoch_ms() - 180_000;
    let dead_pid = exited_process_id();
    fs::write(
        &status_path,
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "status": "refreshing",
            "project_root": producer_path_text(project),
            "phase": "incremental_index",
            "pid": dead_pid,
            "started_at_epoch_ms": old_started,
            "updated_at_epoch_ms": old_started,
            "last_failure_reason": null
        }))
        .expect("status json"),
    )
    .expect("write stale local refresh status");
    fs::write(
        &lock_path,
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "project_root": producer_path_text(project),
            "pid": dead_pid,
            "started_at_epoch_ms": old_started,
            "token": "stale"
        }))
        .expect("lock json"),
    )
    .expect("write stale local refresh lock");
}

#[test]
fn reconcile_before_enqueue_returns_active_repair_when_live() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let run_id = "shared-agent";
    write_ready_repair_status_file(
        project.path(),
        run_id,
        std::process::id(),
        now_epoch_ms(),
        "Qdrant finalize",
    );

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some(run_id), "9.9.9");

    assert_eq!(reconciliation.status, "active_repair");
    assert!(!reconciliation.cleanup_performed);
    let active = reconciliation
        .active_repair
        .expect("active repair operation");
    assert_eq!(active.status, "running");
    assert_eq!(active.phase.as_deref(), Some("Qdrant finalize"));
    assert!(reconciliation.abandoned_repairs.is_empty());
}

#[test]
fn reconcile_before_enqueue_cleans_abandoned_repair_for_dead_pid() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let run_id = "shared-agent";
    let status_path = write_ready_repair_status_file(
        project.path(),
        run_id,
        exited_process_id(),
        now_epoch_ms(),
        "graph artifact",
    );

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some(run_id), "9.9.9");

    assert_eq!(reconciliation.status, "stale_state_cleaned");
    assert!(reconciliation.cleanup_performed);
    assert!(reconciliation.active_repair.is_none());
    assert_eq!(reconciliation.abandoned_repairs.len(), 1);
    assert_eq!(
        reconciliation.abandoned_repairs[0].status,
        "abandoned_cleaned"
    );
    assert!(
        !status_path.exists(),
        "abandoned status file should be removed"
    );
}

#[test]
fn reconcile_before_enqueue_for_sidecar_keeps_abandoned_cleanup_in_retained_root() {
    let project = tempdir().expect("project");
    let retained_cache = tempdir().expect("retained cache");
    let mutable_cache = tempdir().expect("mutable cache");
    let run_id = "shared-agent";
    let retained_sidecar = crate::sidecar_runtime::for_project_with_run_id_in_cache(
        Some(project.path()),
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
        retained_cache.path(),
    );
    let mutable_sidecar = crate::sidecar_runtime::for_project_with_run_id_in_cache(
        Some(project.path()),
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
        mutable_cache.path(),
    );
    let status_path = retained_sidecar
        .layout
        .state_file
        .with_file_name("ready-repair-status.json");
    fs::create_dir_all(status_path.parent().expect("status parent")).expect("create status parent");
    fs::write(
        &status_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "status": "repairing",
            "project_root": producer_path_text(project.path()),
            "profile": "agent",
            "run_id": run_id,
            "namespace": retained_sidecar.namespace,
            "compose_project": retained_sidecar.compose_project,
            "phase": "graph artifact",
            "pid": exited_process_id(),
            "started_at_epoch_ms": now_epoch_ms(),
            "updated_at_epoch_ms": now_epoch_ms()
        }))
        .expect("status json"),
    )
    .expect("write retained status");

    let reconciliation = reconcile_before_enqueue_for_sidecar(
        project.path(),
        mutable_cache.path(),
        &retained_sidecar,
        "9.9.9",
    );

    assert_eq!(reconciliation.status, "stale_state_cleaned");
    assert_eq!(reconciliation.abandoned_repairs.len(), 1);
    assert!(!status_path.exists());
    assert!(
        retained_sidecar
            .layout
            .state_file
            .with_file_name("ready-repair-result.json")
            .exists()
    );
    assert!(
        !mutable_sidecar
            .layout
            .state_file
            .with_file_name("ready-repair-result.json")
            .exists()
    );
}

#[test]
fn reconcile_before_enqueue_reports_clean_when_empty() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some("shared-agent"), "9.9.9");

    assert_eq!(reconciliation.status, "clean");
    assert!(!reconciliation.cleanup_performed);
    assert!(reconciliation.active_repair.is_none());
    assert!(reconciliation.abandoned_repairs.is_empty());
    assert!(reconciliation.local_refresh_cleanups.is_empty());
}

#[test]
fn reconcile_before_enqueue_cleans_stale_local_refresh() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    write_stale_local_refresh(cache.path(), project.path());

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some("shared-agent"), "9.9.9");

    assert_eq!(reconciliation.status, "stale_state_cleaned");
    assert!(reconciliation.cleanup_performed);
    assert_eq!(reconciliation.local_refresh_cleanups.len(), 1);
    assert_eq!(
        reconciliation.local_refresh_cleanups[0].status,
        "stale_cleaned"
    );
    assert!(!cache.path().join("local-refresh-status.json").exists());
    assert!(!cache.path().join("local-refresh.lock").exists());
    assert!(
        cache.path().join("local-refresh-state.guard").exists(),
        "compatibility cleanup must preserve the persistent guard inode"
    );
}

#[test]
fn reconcile_before_enqueue_reports_live_stale_ready_repair_without_cleanup() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let run_id = "shared-agent";
    let old = now_epoch_ms() - 180_000;
    let status_path = write_ready_repair_status_file(
        project.path(),
        run_id,
        std::process::id(),
        old,
        "Embedding documents",
    );

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some(run_id), "9.9.9");

    assert_eq!(reconciliation.status, "orphan_unresolved");
    assert!(!reconciliation.cleanup_performed);
    assert!(reconciliation.abandoned_repairs.is_empty());
    assert!(
        status_path.exists(),
        "live stale repair status should remain for the owner to update or clear"
    );
    assert!(
        reconciliation
            .unresolved_orphan_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("live_ready_repair_heartbeat_stale")),
        "{reconciliation:?}"
    );
}

#[test]
fn reconcile_before_enqueue_reports_live_stale_local_refresh_without_cleanup() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let old = now_epoch_ms() - 180_000;
    fs::write(
        cache.path().join("local-refresh-status.json"),
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "status": "refreshing",
            "project_root": producer_path_text(project.path()),
            "phase": "incremental_index",
            "pid": std::process::id(),
            "started_at_epoch_ms": old,
            "updated_at_epoch_ms": old,
            "last_failure_reason": null
        }))
        .expect("status json"),
    )
    .expect("write live stale local refresh status");
    fs::write(
        cache.path().join("local-refresh.lock"),
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "project_root": producer_path_text(project.path()),
            "pid": std::process::id(),
            "started_at_epoch_ms": old,
            "token": "live-stale"
        }))
        .expect("lock json"),
    )
    .expect("write live stale local refresh lock");

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some("shared-agent"), "9.9.9");

    assert_eq!(reconciliation.status, "orphan_unresolved");
    assert!(!reconciliation.cleanup_performed);
    assert_eq!(reconciliation.local_refresh_cleanups.len(), 1);
    assert_eq!(
        reconciliation.local_refresh_cleanups[0].status,
        "stale_live"
    );
    assert!(cache.path().join("local-refresh-status.json").exists());
    assert!(cache.path().join("local-refresh.lock").exists());
    assert_eq!(
        reconciliation.unresolved_orphan_reason.as_deref(),
        Some("local_refresh_cleanup_blocked:live_status_heartbeat_stale")
    );
}

#[test]
fn reconcile_before_enqueue_preserves_renewed_live_local_refresh_owner() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let old = now_epoch_ms() - 180_000;
    fs::write(
        cache.path().join("local-refresh-status.json"),
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "status": "refreshing",
            "project_root": producer_path_text(project.path()),
            "phase": "incremental_index",
            "pid": std::process::id(),
            "owner_token": "renewed-live",
            "started_at_epoch_ms": old,
            "updated_at_epoch_ms": now_epoch_ms(),
            "last_failure_reason": null
        }))
        .expect("status json"),
    )
    .expect("write renewed local refresh status");
    fs::write(
        cache.path().join("local-refresh.lock"),
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "project_root": producer_path_text(project.path()),
            "pid": std::process::id(),
            "started_at_epoch_ms": old,
            "token": "renewed-live"
        }))
        .expect("lock json"),
    )
    .expect("write renewed local refresh lock");

    let reconciliation =
        reconcile_before_enqueue(project.path(), cache.path(), Some("shared-agent"), "9.9.9");

    assert!(reconciliation.local_refresh_cleanups.is_empty());
    assert_ne!(reconciliation.status, "orphan_unresolved");
    assert!(cache.path().join("local-refresh-status.json").exists());
    assert!(cache.path().join("local-refresh.lock").exists());
}

#[test]
fn machine_resource_lock_contention_has_single_winner_across_threads() {
    let project = tempdir().expect("temp project");
    let resource = unique_resource("lock-contention-single-winner");
    cleanup_machine_resource(&resource);
    let scope = test_scope(project.path(), "contender");
    let contender_count = 8;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(contender_count));
    let mut handles = Vec::new();

    for _ in 0..contender_count {
        let resource = resource.clone();
        let scope = scope.clone();
        let barrier = barrier.clone();
        handles.push(spawn_with_test_broker_root(move || {
            barrier.wait();
            try_acquire_machine_resource_lock(&resource, &scope).expect("try lock")
        }));
    }

    let outcomes: Vec<BrokerMachineResourceLockAttempt> = handles
        .into_iter()
        .map(|handle| handle.join().expect("lock contender thread"))
        .collect();
    let winners = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, BrokerMachineResourceLockAttempt::Acquired(_)))
        .count();
    let busy = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, BrokerMachineResourceLockAttempt::Busy(_)))
        .count();

    assert_eq!(
        winners, 1,
        "machine lock contention must have exactly one winner"
    );
    assert_eq!(busy, contender_count - 1);
    drop(outcomes);
    cleanup_machine_resource(&resource);
}

#[test]
fn refresh_broker_snapshot_final_success_omits_running_ops_after_repair_cleared() {
    // Focused seam for "final success snapshot after lock/status release":
    // while durable repair status exists, refresh reports a running op; after
    // clear (as Drop of ReadyRepairProgress does before the outer final
    // refresh), the success snapshot no longer carries running repair ops.
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let run_id = "shared-agent";
    let sidecar = test_sidecar_runtime(
        project.path(),
        codestory_retrieval::SidecarProfile::Agent,
        Some(run_id),
    );
    let started_at = now_epoch_ms();
    let runtime_identity = gpu_runtime_identity(project.path(), started_at);
    crate::ready_repair_status::write_ready_repair_status(
        &sidecar,
        project.path(),
        "readiness check",
        started_at,
        std::process::id(),
    )
    .expect("write live repair status");

    let during = refresh_broker_snapshot_with_runtime_identity(
        BrokerSnapshotInput {
            project_root: project.path().to_path_buf(),
            cache_root: cache.path().to_path_buf(),
            agent_run_id: Some(run_id.to_string()),
            cli_version: "9.9.9".to_string(),
            gpu_proof: Some(verified_gpu_proof_input(Some(true))),
            reconciliation: None,
        },
        Some(&runtime_identity),
    );
    assert!(
        during
            .operations
            .iter()
            .any(|op| op.operation_kind == "agent_repair" && op.status == "running"),
        "in-flight repair must appear before status clear: {during:?}"
    );

    crate::ready_repair_status::clear_ready_repair_status(&sidecar, started_at, std::process::id());

    let after = refresh_broker_snapshot_with_runtime_identity(
        BrokerSnapshotInput {
            project_root: project.path().to_path_buf(),
            cache_root: cache.path().to_path_buf(),
            agent_run_id: Some(run_id.to_string()),
            cli_version: "9.9.9".to_string(),
            gpu_proof: Some(verified_gpu_proof_input(Some(true))),
            reconciliation: None,
        },
        Some(&runtime_identity),
    );
    assert!(
        after
            .operations
            .iter()
            .all(|op| !(op.operation_kind == "agent_repair" && op.status == "running")),
        "final success snapshot after repair clear must not keep running ops: {after:?}"
    );
    assert_eq!(after.persistence_status, "persisted");
    let proof = after.gpu_proof.expect("gpu proof on final snapshot");
    assert_eq!(proof.embed_smoke_ok, Some(true));
    assert_eq!(proof.embed_smoke_ms, Some(12));
}

#[test]
fn observe_broker_snapshot_is_stable_and_does_not_publish() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let canonical_root_hash = codestory_workspace::workspace_id_v3_for_root(project.path());
    let snapshot_path = broker_snapshot_path(&canonical_root_hash);
    let _ = fs::remove_file(&snapshot_path);

    let observe = || {
        observe_broker_snapshot(BrokerSnapshotInput {
            project_root: project.path().to_path_buf(),
            cache_root: cache.path().to_path_buf(),
            agent_run_id: Some("shared-agent".to_string()),
            cli_version: "9.9.9".to_string(),
            gpu_proof: None,
            reconciliation: None,
        })
    };

    let first = observe();
    let second = observe();

    assert_eq!(first, second);
    assert_eq!(first.updated_at_epoch_ms, 0);
    assert_eq!(first.persistence_status, "observed");
    assert!(!snapshot_path.exists());
}

#[test]
fn broker_unit_test_paths_use_injected_machine_state_root() {
    let root = broker_cache_root();

    assert!(
        root.starts_with(std::env::temp_dir()),
        "broker test root must be process-local: {}",
        root.display()
    );
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        assert!(
            !root.starts_with(std::path::PathBuf::from(home)),
            "broker test root must not use the platform cache: {}",
            root.display()
        );
    }
    assert!(machine_resource_lock_path(NATIVE_EMBEDDING_RESOURCE).starts_with(&root));
}

#[test]
fn observe_broker_snapshot_reuses_matching_persisted_transition() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let input = || BrokerSnapshotInput {
        project_root: project.path().to_path_buf(),
        cache_root: cache.path().to_path_buf(),
        agent_run_id: Some("shared-agent".to_string()),
        cli_version: "9.9.9".to_string(),
        gpu_proof: None,
        reconciliation: None,
    };

    let published = refresh_broker_snapshot(input());
    let observed = observe_broker_snapshot(input());

    assert_eq!(observed.updated_at_epoch_ms, published.updated_at_epoch_ms);
    assert_eq!(observed.persistence_status, "persisted");
    assert!(observed.persistence_error.is_none());
}

#[test]
fn observe_broker_snapshot_preserves_verified_smoke_for_current_runtime() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let runtime_identity = gpu_runtime_identity(project.path(), now_epoch_ms());
    let input = |embed_smoke_ok| BrokerSnapshotInput {
        project_root: project.path().to_path_buf(),
        cache_root: cache.path().to_path_buf(),
        agent_run_id: Some("shared-agent".to_string()),
        cli_version: "9.9.9".to_string(),
        gpu_proof: Some(verified_gpu_proof_input(embed_smoke_ok)),
        reconciliation: None,
    };

    let refreshed =
        refresh_broker_snapshot_with_runtime_identity(input(Some(true)), Some(&runtime_identity));
    let observed =
        observe_broker_snapshot_with_runtime_identity(input(None), Some(&runtime_identity));

    assert_eq!(
        refreshed.gpu_proof.as_ref().unwrap().proof_status,
        "verified"
    );
    assert_eq!(
        observed.gpu_proof.as_ref().unwrap().proof_status,
        "verified"
    );
    assert_eq!(
        observed.gpu_proof.as_ref().unwrap().embed_smoke_ok,
        Some(true)
    );
    assert_eq!(
        observed.gpu_proof.as_ref().unwrap().embed_smoke_ms,
        Some(12)
    );
    assert_eq!(observed.updated_at_epoch_ms, refreshed.updated_at_epoch_ms);
    assert_eq!(observed.persistence_status, "persisted");
}

#[test]
fn failed_smoke_replaces_verified_proof_for_current_runtime() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let runtime_identity = gpu_runtime_identity(project.path(), now_epoch_ms());
    let input = |embed_smoke_ok| BrokerSnapshotInput {
        project_root: project.path().to_path_buf(),
        cache_root: cache.path().to_path_buf(),
        agent_run_id: Some("shared-agent".to_string()),
        cli_version: "9.9.9".to_string(),
        gpu_proof: Some(verified_gpu_proof_input(embed_smoke_ok)),
        reconciliation: None,
    };

    refresh_broker_snapshot_with_runtime_identity(input(Some(true)), Some(&runtime_identity));
    let failed =
        refresh_broker_snapshot_with_runtime_identity(input(Some(false)), Some(&runtime_identity));
    let proof = failed.gpu_proof.expect("failed smoke proof");

    assert_eq!(proof.proof_status, "gpu_unverified");
    assert!(!proof.meaningful_accelerator_work_proven);
    assert_eq!(proof.embed_smoke_ok, Some(false));
    assert_eq!(proof.runtime_identity, None);
}

#[test]
fn observe_broker_snapshot_invalidates_verified_smoke_for_changed_runtime_identity() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let original_runtime = gpu_runtime_identity(project.path(), now_epoch_ms());
    let mut changed_runtime = original_runtime.clone();
    changed_runtime.started_at_epoch_ms += 1;
    let input = |embed_smoke_ok| BrokerSnapshotInput {
        project_root: project.path().to_path_buf(),
        cache_root: cache.path().to_path_buf(),
        agent_run_id: Some("shared-agent".to_string()),
        cli_version: "9.9.9".to_string(),
        gpu_proof: Some(verified_gpu_proof_input(embed_smoke_ok)),
        reconciliation: None,
    };

    let refreshed =
        refresh_broker_snapshot_with_runtime_identity(input(Some(true)), Some(&original_runtime));
    let observed =
        observe_broker_snapshot_with_runtime_identity(input(None), Some(&changed_runtime));

    assert_eq!(
        refreshed.gpu_proof.as_ref().unwrap().proof_status,
        "verified"
    );
    let proof = observed.gpu_proof.expect("observed gpu proof");
    assert_eq!(proof.proof_status, "gpu_unverified");
    assert!(!proof.meaningful_accelerator_work_proven);
    assert_eq!(proof.embed_smoke_ok, None);
    assert_eq!(proof.runtime_identity, None);
    assert_eq!(observed.persistence_status, "observed");
}

#[test]
fn legacy_snapshot_observation_is_read_only_and_refresh_migrates_to_v3() {
    let project = tempdir().expect("project");
    let cache = tempdir().expect("cache");
    let canonical_root_hash = hash_text(&clean_path_text(project.path()));
    let legacy_snapshot_path = broker_snapshot_path(&canonical_root_hash);
    let snapshot_path = broker_snapshot_path(&codestory_workspace::workspace_id_v3_for_root(
        project.path(),
    ));
    fs::create_dir_all(legacy_snapshot_path.parent().expect("snapshot parent"))
        .expect("create snapshot parent");
    let mut legacy = sample_snapshot(project.path());
    legacy.schema_version = LEGACY_BROKER_SCHEMA_VERSION;
    legacy.identity = None;
    legacy.project_id = format!("codestory-{}", &canonical_root_hash[..16]);
    legacy.canonical_root_hash = canonical_root_hash;
    let legacy_json = serde_json::to_vec_pretty(&legacy).expect("serialize legacy snapshot");
    fs::write(&legacy_snapshot_path, &legacy_json).expect("write legacy snapshot");
    let input = || BrokerSnapshotInput {
        project_root: project.path().to_path_buf(),
        cache_root: cache.path().to_path_buf(),
        agent_run_id: Some("shared-agent".to_string()),
        cli_version: "9.9.9".to_string(),
        gpu_proof: None,
        reconciliation: None,
    };

    let observed = observe_broker_snapshot(input());
    assert_eq!(observed.schema_version, BROKER_SCHEMA_VERSION);
    assert_eq!(observed.persistence_status, "observed");
    assert_eq!(
        fs::read(&legacy_snapshot_path).expect("legacy snapshot remains"),
        legacy_json,
        "observational reads must not migrate persisted state"
    );

    let refreshed = refresh_broker_snapshot(input());
    assert_eq!(refreshed.schema_version, BROKER_SCHEMA_VERSION);
    let migrated: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).expect("read migrated snapshot"))
            .expect("parse migrated snapshot");
    assert_eq!(migrated["schema_version"], BROKER_SCHEMA_VERSION);
    assert_eq!(
        migrated["identity"]["workspace_id"],
        codestory_workspace::workspace_id_v3_for_root(project.path())
    );
    assert_eq!(
        fs::read(&legacy_snapshot_path).expect("legacy snapshot remains isolated"),
        legacy_json
    );
    let _ = fs::remove_file(snapshot_path);
    let _ = fs::remove_file(legacy_snapshot_path);
}

#[test]
fn legacy_machine_lock_derives_identity_without_rewriting() {
    let project = tempdir().expect("project");
    let resource = unique_resource("legacy-identity");
    cleanup_machine_resource(&resource);
    let mut scope = test_scope(project.path(), "shared-agent");
    scope.schema_version = LEGACY_BROKER_SCHEMA_VERSION;
    scope.identity = None;
    scope.canonical_root_hash = hash_text(&clean_path_text(project.path()));
    scope.project_id = format!("codestory-{}", &scope.canonical_root_hash[..16]);
    let lock = BrokerMachineResourceLockFile {
        schema_version: LEGACY_BROKER_SCHEMA_VERSION,
        resource: resource.clone(),
        operation_id: "legacy-operation".to_string(),
        scope,
        pid: std::process::id(),
        started_at_epoch_ms: now_epoch_ms(),
        process_start_identity: None,
        token: "legacy-token".to_string(),
        native_embedding_launch: None,
        native_embedding_quarantine_reason: None,
    };
    let path = machine_resource_lock_path(&resource);
    fs::create_dir_all(path.parent().expect("lock parent")).expect("create lock parent");
    let legacy_json = serde_json::to_vec_pretty(&lock).expect("serialize legacy lock");
    fs::write(&path, &legacy_json).expect("write legacy lock");

    let parsed = read_machine_resource_lock_file(&path).expect("read legacy machine lock");
    let identity = effective_scope_identity(&parsed.scope).expect("derive legacy identity");
    assert_eq!(
        identity.workspace_id,
        codestory_workspace::workspace_id_v3_for_root(project.path())
    );
    assert_eq!(fs::read(&path).expect("lock remains"), legacy_json);
    cleanup_machine_resource(&resource);
}

#[test]
fn v2_machine_lock_maps_to_v3_without_rewriting() {
    let project = tempdir().expect("project");
    let resource = unique_resource("v2-identity");
    cleanup_machine_resource(&resource);
    let legacy_identity = codestory_workspace::project_identity_v2(project.path());
    let mut scope = test_scope(project.path(), "shared-agent");
    scope.schema_version = BROKER_SCHEMA_VERSION_V2;
    scope.identity = Some(
        serde_json::from_value(serde_json::to_value(&legacy_identity).expect("v2 identity json"))
            .expect("v2 identity compatibility"),
    );
    scope.project_id = legacy_identity.project_id;
    scope.canonical_root_hash = hash_text(&clean_path_text(project.path()));
    let lock = BrokerMachineResourceLockFile {
        schema_version: MACHINE_LOCK_SCHEMA_VERSION_V2,
        resource: resource.clone(),
        operation_id: broker_operation_id(&scope),
        scope,
        pid: std::process::id(),
        started_at_epoch_ms: now_epoch_ms(),
        process_start_identity: None,
        token: "v2-token".to_string(),
        native_embedding_launch: None,
        native_embedding_quarantine_reason: None,
    };
    let path = machine_resource_lock_path(&resource);
    fs::create_dir_all(path.parent().expect("lock parent")).expect("create lock parent");
    let legacy_json = serde_json::to_vec_pretty(&lock).expect("serialize v2 lock");
    fs::write(&path, &legacy_json).expect("write v2 lock");

    let parsed = read_machine_resource_lock_file(&path).expect("read v2 machine lock");
    let identity = effective_scope_identity(&parsed.scope).expect("map v2 identity");
    assert_eq!(
        identity.workspace_id,
        codestory_workspace::workspace_id_v3_for_root(project.path())
    );
    assert_eq!(
        identity.project_identity_schema_version,
        codestory_workspace::PROJECT_IDENTITY_V3_SCHEMA_VERSION
    );
    assert_eq!(fs::read(&path).expect("lock remains"), legacy_json);
    cleanup_machine_resource(&resource);
}
