use anyhow::{Context, Result};
use codestory_contracts::api::IndexMode;
use std::time::Duration;

use codestory_retrieval::{
    BootstrapStorageScope, FinalizeIndexOutcome, ProjectQdrantRepairOutcome, QueryRequest,
    RetrievalIndexManifest, RetrievalStatusReport, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED,
    SidecarProfile, SidecarRuntimeConfig, bootstrap_sidecars_with_runtime,
    sidecar_down_for_runtime, sidecar_up_with_runtime_preserving_launch,
    strict_sidecar_status_for_runtime,
};

use crate::args::{
    CliSidecarProfile, OutputFormat, RefreshMode, RetrievalAction, RetrievalBootstrapCommand,
    RetrievalCommand, RetrievalIndexCommand, RetrievalInventoryCommand, RetrievalQueryCommand,
    RetrievalSidecarStateCommand, RetrievalStatusCommand,
};
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error, resolve_refresh_request};

pub(crate) fn run_retrieval(cmd: RetrievalCommand) -> Result<()> {
    match cmd.action {
        RetrievalAction::Bootstrap(bootstrap_cmd) => run_retrieval_bootstrap(bootstrap_cmd),
        RetrievalAction::Up(up_cmd) => run_retrieval_up(up_cmd),
        RetrievalAction::Down(down_cmd) => run_retrieval_down(down_cmd),
        RetrievalAction::Status(status_cmd) => run_retrieval_status(status_cmd),
        RetrievalAction::Inventory(inventory_cmd) => run_retrieval_inventory(inventory_cmd),
        RetrievalAction::Index(index_cmd) => run_retrieval_index(index_cmd),
        RetrievalAction::Query(query_cmd) => run_retrieval_query(query_cmd),
    }
}

fn run_retrieval_bootstrap(cmd: RetrievalBootstrapCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let storage_scope = BootstrapStorageScope::from_parts(
        Some(runtime.project_root.as_path()),
        Some(runtime.storage_path.as_path()),
        Some(runtime.cache_root.as_path()),
    );
    let sidecar_profile = cmd.profile;
    let sidecar = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        sidecar_profile.into(),
        cmd.run_id.as_deref(),
    );
    let broker_scope = crate::readiness_broker::operation_scope(
        &runtime.project_root,
        sidecar.profile.as_str(),
        sidecar.run_id.as_deref(),
        "retrieval_bootstrap",
        env!("CARGO_PKG_VERSION"),
    );
    let (report, project_qdrant_repair, status) =
        crate::readiness_broker::run_with_native_embedding_lease_lifecycle(
            crate::readiness_broker::NativeEmbeddingLeaseLifecycleParams {
                scope: &broker_scope,
                sidecar: &sidecar,
                wait: Duration::from_secs(30),
                poll: Duration::from_millis(250),
                bootstrap_context: "retrieval bootstrap",
                sidecar_cleanup_label: "retrieval sidecar",
            },
            |allow_native_embedding_spawn| {
                bootstrap_sidecars_with_runtime(
                    &sidecar,
                    Some(&runtime.project_root),
                    &storage_scope,
                    cmd.compose_file.as_deref(),
                    cmd.skip_compose,
                    Duration::from_secs(cmd.wait_secs),
                    allow_native_embedding_spawn,
                )
            },
            |report| &report.state,
            |report| {
                let project_qdrant_repair =
                    codestory_retrieval::repair_project_qdrant_collection_for_runtime(
                        &runtime.project_root,
                        &runtime.storage_path,
                        &sidecar,
                    )
                    .context("retrieval project qdrant repair")?;
                let status = strict_sidecar_status_for_runtime(
                    &runtime.project_root,
                    Some(&runtime.storage_path),
                    sidecar.clone(),
                )
                .context("retrieval status after bootstrap")?;
                Ok((report, project_qdrant_repair, status))
            },
        )?;
    let readiness_broker = crate::readiness_broker::refresh_broker_snapshot(
        crate::readiness_broker::BrokerSnapshotInput {
            project_root: runtime.project_root.clone(),
            cache_root: runtime.cache_root.clone(),
            agent_run_id: sidecar.run_id.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            gpu_proof: Some(broker_gpu_proof_input_from_status(&status)),
            reconciliation: None,
        },
    );
    emit_retrieval_bootstrap(
        cmd.format,
        &report,
        project_qdrant_repair.as_ref(),
        &status,
        &readiness_broker,
        cmd.output_file.as_deref(),
    )
}

fn run_retrieval_up(cmd: RetrievalSidecarStateCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let sidecar = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        cmd.profile.into(),
        cmd.run_id.as_deref(),
    );
    let state =
        sidecar_up_with_runtime_preserving_launch(&sidecar, None).context("retrieval up")?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

fn run_retrieval_down(cmd: RetrievalSidecarStateCommand) -> Result<()> {
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let sidecar = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        cmd.profile.into(),
        cmd.run_id.as_deref(),
    );
    let native_embedding_launch =
        crate::readiness_broker::native_embedding_launch_from_sidecar_state_file(&sidecar)?;
    sidecar_down_for_runtime(&sidecar).context("retrieval down")?;
    if let Some(launch) = native_embedding_launch.as_ref() {
        crate::readiness_broker::release_machine_resource_lock_for_native_launch(
            crate::readiness_broker::NATIVE_EMBEDDING_RESOURCE,
            launch,
        )
        .context("release native embedding broker lock")?;
    }
    println!("retrieval sidecar state cleared");
    Ok(())
}

fn broker_gpu_proof_input_from_status(
    status: &RetrievalStatusReport,
) -> crate::readiness_broker::BrokerGpuProofInput {
    crate::broker_gpu_proof_input_from_report(status)
}

pub(crate) fn run_retrieval_status(cmd: RetrievalStatusCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let profile = cmd
        .profile
        .or_else(|| cmd.run_id.as_ref().map(|_| CliSidecarProfile::Agent));
    let mut agent_run_id = None;
    let report = if let Some(profile) = profile {
        let sidecar = runtime.sidecar.with_profile_and_run_id(
            Some(&runtime.project_root),
            profile.into(),
            cmd.run_id.as_deref(),
        );
        if profile == CliSidecarProfile::Agent {
            agent_run_id = sidecar.run_id.clone();
        }
        codestory_retrieval::strict_sidecar_status_for_runtime(
            &runtime.project_root,
            Some(&runtime.storage_path),
            sidecar,
        )
    } else {
        strict_sidecar_status_for_runtime(
            &runtime.project_root,
            Some(&runtime.storage_path),
            runtime.sidecar.clone(),
        )
    }
    .context("retrieval status")?;
    let readiness_broker = crate::readiness_broker::observe_broker_snapshot(
        crate::readiness_broker::BrokerSnapshotInput {
            project_root: runtime.project_root.clone(),
            cache_root: runtime.cache_root.clone(),
            agent_run_id,
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            gpu_proof: Some(broker_gpu_proof_input_from_status(&report)),
            reconciliation: None,
        },
    );
    emit_retrieval_status(
        cmd.format,
        &report,
        &readiness_broker,
        cmd.output_file.as_deref(),
    )
}

pub(crate) fn run_retrieval_inventory(cmd: RetrievalInventoryCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    if cmd.apply {
        let report = codestory_retrieval::sidecar_gc_apply_with_storage(
            &runtime.project_root,
            &runtime.storage_path,
        )
        .context("retrieval inventory apply")?;
        return emit_retrieval_gc(cmd.format, &report, cmd.output_file.as_deref());
    }
    let report = codestory_retrieval::sidecar_inventory_with_storage(
        &runtime.project_root,
        &runtime.storage_path,
    )
    .context("retrieval inventory")?;
    emit_retrieval_inventory(cmd.format, &report, cmd.output_file.as_deref())
}

fn run_retrieval_query(cmd: RetrievalQueryCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let result = codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
        QueryRequest {
            project_root: &runtime.project_root,
            storage_path: &runtime.storage_path,
            query: &cmd.query,
            budget_ms: cmd.budget_ms,
            cancelled: None,
        },
        &mut codestory_retrieval::RetrievalCache::new(),
        &runtime.sidecar,
    )
    .context("retrieval query")?;
    emit_retrieval_query(cmd.format, &result, cmd.output_file.as_deref())
}

fn run_retrieval_index(cmd: RetrievalIndexCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let sidecar_profile = cmd.profile.unwrap_or(CliSidecarProfile::Local);
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let sidecar = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        sidecar_profile.into(),
        cmd.run_id.as_deref(),
    );
    let summary = runtime.open_project_summary()?;
    let refresh_mode = resolve_refresh_request(cmd.refresh, &summary);
    ensure_retrieval_index_embedding_policy(&sidecar)?;
    if sidecar.profile == SidecarProfile::Agent {
        codestory_retrieval::ensure_embedding_accelerator_smoke_for_runtime(&sidecar)
            .context("retrieval index GPU pre-repair gate")?;
    }
    run_retrieval_index_refresh(&runtime, cmd.refresh, refresh_mode)?;
    let outcome =
        finalize_retrieval_index_for_sidecar_runtime(&runtime, &sidecar).or_else(|error| {
            if !retrieval_index_should_retry_full_refresh(cmd.refresh, &error) {
                return Err(error);
            }
            runtime
                .index
                .run_indexing_blocking(IndexMode::Full)
                .map_err(map_api_error)?;
            finalize_retrieval_index_for_sidecar_runtime(&runtime, &sidecar)
                .context("retrieval index finalize after semantic-doc contract repair")
        })?;
    ensure_local_profile_handoff(&runtime, &sidecar, &outcome)?;
    emit_retrieval_index(cmd.format, &outcome, cmd.output_file.as_deref())
}

fn ensure_retrieval_index_embedding_policy(sidecar: &SidecarRuntimeConfig) -> Result<()> {
    codestory_retrieval::ensure_product_embedding_backend_for_runtime(sidecar)
        .context("retrieval index embedding device policy")
}

fn run_retrieval_index_refresh(
    runtime: &RuntimeContext,
    requested_refresh: RefreshMode,
    refresh_mode: Option<IndexMode>,
) -> Result<()> {
    let Some(mode) = refresh_mode else {
        return Ok(());
    };
    runtime
        .index
        .run_indexing_blocking(mode)
        .map_err(map_api_error)
        .map(|_| ())
        .or_else(|error| {
            if !retrieval_index_should_retry_full_refresh(requested_refresh, &error) {
                return Err(error);
            }
            runtime
                .index
                .run_indexing_blocking(IndexMode::Full)
                .map_err(map_api_error)
                .map(|_| ())
                .context("retrieval index full refresh after semantic-doc contract repair")
        })
}

pub(crate) fn finalize_retrieval_index_for_runtime(
    runtime: &RuntimeContext,
) -> Result<FinalizeIndexOutcome> {
    finalize_retrieval_index_for_sidecar_runtime(runtime, &runtime.sidecar)
}

pub(crate) fn finalize_retrieval_index_for_sidecar_runtime(
    runtime: &RuntimeContext,
    sidecar: &SidecarRuntimeConfig,
) -> Result<FinalizeIndexOutcome> {
    let opened = runtime.ensure_open(crate::args::RefreshMode::None)?;
    ensure_index_ready(&opened, "retrieval index")?;
    codestory_retrieval::finalize_index_for_runtime(
        &runtime.project_root,
        &runtime.storage_path,
        sidecar,
    )
    .context("retrieval index finalize")
}

fn ensure_local_profile_handoff(
    runtime: &RuntimeContext,
    indexed_sidecar: &SidecarRuntimeConfig,
    outcome: &FinalizeIndexOutcome,
) -> Result<()> {
    if indexed_sidecar.profile != SidecarProfile::Local {
        return Ok(());
    }
    let default_sidecar = runtime.sidecar.clone();
    if let Some(mismatch) = sidecar_runtime_mismatch(indexed_sidecar, &default_sidecar) {
        anyhow::bail!("{mismatch}");
    }
    let status = strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        default_sidecar,
    )
    .context("retrieval local/default status after index")?;
    if status.retrieval_mode != "full" {
        anyhow::bail!(
            "retrieval profile handoff failed: local/default status after index is mode={} reason={}; indexed_project_id={} sidecar={}",
            status.retrieval_mode,
            status.degraded_reason.as_deref().unwrap_or("unknown"),
            outcome.project_id,
            format_sidecar_runtime(indexed_sidecar)
        );
    }
    Ok(())
}

fn sidecar_runtime_mismatch(
    indexed: &SidecarRuntimeConfig,
    default: &SidecarRuntimeConfig,
) -> Option<String> {
    let same_paths = indexed.profile == default.profile
        && indexed.namespace == default.namespace
        && indexed.layout.lexical_data_dir == default.layout.lexical_data_dir
        && indexed.layout.qdrant_data_dir == default.layout.qdrant_data_dir
        && indexed.layout.scip_artifacts_root == default.layout.scip_artifacts_root
        && indexed.layout.state_file == default.layout.state_file;
    (!same_paths).then(|| {
        format!(
            "retrieval profile handoff mismatch: indexed local artifacts use {}; default bare retrieval resolves to {}; retrieval index must not report success until local/default namespace and paths match",
            format_sidecar_runtime(indexed),
            format_sidecar_runtime(default)
        )
    })
}

pub(crate) fn format_sidecar_runtime(runtime: &SidecarRuntimeConfig) -> String {
    format!(
        "profile={} namespace={} state={} lexical={} qdrant={} scip={}",
        runtime.profile.as_str(),
        runtime.namespace,
        runtime.layout.state_file.display(),
        runtime.layout.lexical_data_dir.display(),
        runtime.layout.qdrant_data_dir.display(),
        runtime.layout.scip_artifacts_root.display()
    )
}

fn retrieval_index_should_retry_full_refresh(
    requested_refresh: RefreshMode,
    error: &anyhow::Error,
) -> bool {
    requested_refresh == RefreshMode::Auto
        && error_chain_contains(error, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED)
}

fn error_chain_contains(error: &anyhow::Error, needle: &str) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains(needle))
}

fn preflight_output(output_file: Option<&std::path::Path>) -> Result<()> {
    if let Some(path) = output_file {
        validate_output_file_parent(path)?;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct RetrievalIndexOutput<'a> {
    manifest: &'a RetrievalIndexManifest,
    degraded_modes: &'a [String],
    qdrant_stubbed: bool,
    scip_stubbed: bool,
    generation_retention_plan: &'a codestory_retrieval::GenerationRetentionPlan,
    generation_retention: &'a codestory_retrieval::GenerationRetentionApplyReport,
}

fn emit_retrieval_index(
    format: OutputFormat,
    outcome: &FinalizeIndexOutcome,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let payload = RetrievalIndexOutput {
        manifest: &outcome.manifest,
        degraded_modes: &outcome.degraded_modes,
        qdrant_stubbed: outcome.qdrant_stubbed,
        scip_stubbed: outcome.scip_stubbed,
        generation_retention_plan: &outcome.generation_retention_plan,
        generation_retention: &outcome.generation_retention,
    };
    let markdown = format!(
        "# Retrieval index\n\n- project_id: `{}`\n- lexical_version: `{}`\n- qdrant_collection: `{}`\n- scip_revision: {:?}\n- degraded_modes: {:?}\n- retention_retained_bytes: {}\n- retention_reclaimable_bytes: {}\n- retention_removed_bytes: {}\n- retention_remaining_reclaimable_bytes: {}\n- retention_pruning_suppressed: {}\n",
        payload.manifest.project_id,
        payload.manifest.lexical_version,
        payload.manifest.qdrant_collection,
        payload.manifest.scip_revision,
        payload.degraded_modes,
        payload.generation_retention.retained_bytes,
        payload.generation_retention.reclaimable_bytes,
        payload.generation_retention.removed_bytes,
        payload.generation_retention.remaining_reclaimable_bytes,
        payload.generation_retention.pruning_suppressed,
    );
    emit(format, &payload, markdown, output_file)
}

fn emit_retrieval_query(
    format: OutputFormat,
    result: &codestory_retrieval::QueryResult,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let top_hit = result
        .hits
        .first()
        .map(|hit| format!("{} ({:.3})", hit.file_path, hit.score))
        .unwrap_or_else(|| "none".into());
    let markdown = format!(
        "# Retrieval query\n\n- query: `{}`\n- shape: `{:?}`\n- retrieval_mode: `{}`\n- hits: {}\n- top: {}\n- elapsed_ms: {}\n",
        result.query,
        result.features.shape,
        result.trace.retrieval_mode,
        result.hits.len(),
        top_hit,
        result.trace.elapsed_ms,
    );
    emit(format, result, markdown, output_file)
}

#[derive(serde::Serialize)]
struct RetrievalBootstrapOutput<'a> {
    compose_started: bool,
    compose_file: Option<&'a str>,
    lexical_ready: bool,
    qdrant_reachable: bool,
    embed_reachable: bool,
    lexical_detail: &'a str,
    qdrant_detail: &'a str,
    embed_detail: &'a str,
    storage_repair: &'a codestory_retrieval::QdrantStorageRepairReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_qdrant_repair: Option<&'a ProjectQdrantRepairOutcome>,
    sidecar_state: &'a codestory_retrieval::SidecarStateFile,
    project_status: &'a RetrievalStatusReport,
    readiness_broker: &'a crate::readiness_broker::ReadinessBrokerSnapshot,
}

fn emit_retrieval_bootstrap(
    format: OutputFormat,
    report: &codestory_retrieval::BootstrapReport,
    project_qdrant_repair: Option<&ProjectQdrantRepairOutcome>,
    status: &RetrievalStatusReport,
    readiness_broker: &crate::readiness_broker::ReadinessBrokerSnapshot,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let compose_path = report
        .compose_file
        .as_ref()
        .map(|path| path.display().to_string());
    let payload = RetrievalBootstrapOutput {
        compose_started: report.compose_started,
        compose_file: compose_path.as_deref(),
        lexical_ready: report.infrastructure.lexical_ready,
        qdrant_reachable: report.infrastructure.qdrant_reachable,
        embed_reachable: report.infrastructure.embed_reachable,
        lexical_detail: &report.infrastructure.lexical_detail,
        qdrant_detail: &report.infrastructure.qdrant_detail,
        embed_detail: &report.infrastructure.embed_detail,
        storage_repair: &report.storage_repair,
        project_qdrant_repair,
        sidecar_state: &report.state,
        project_status: status,
        readiness_broker,
    };
    let repair = &report.storage_repair;
    let overflow_note = if repair.overflow_protected {
        "\n- storage_repair_warning: all collections are protected but count exceeds retention cap; no collections pruned\n"
    } else {
        ""
    };
    let scan_warning = if repair.scan_errors.is_empty() {
        String::new()
    } else {
        format!(
            "\n- storage_repair_scan_warnings: {} (see JSON for details)\n",
            repair.scan_errors.len()
        )
    };
    let prune_suppressed_note = repair
        .prune_suppressed_reason
        .as_deref()
        .map(|reason| {
            if reason
                == codestory_retrieval::PRUNE_SUPPRESSED_POST_PUBLICATION_RETENTION
            {
                return format!(
                    "\n- storage_repair_prune_suppressed: `{reason}` (valid generations are pruned only after a replacement is fully published)\n"
                );
            }
            format!(
                "\n- storage_repair_prune_suppressed: `{reason}` (retention deletes skipped; set CODESTORY_RETRIEVAL_PRUNE_ON_SCAN_ERROR=1 to override)\n"
            )
        })
        .unwrap_or_default();
    let project_repair_note = project_qdrant_repair
        .map(|repair| {
            format!(
                "\n- project_qdrant_repair: collection=`{}` repaired={} points={} skipped_reason={:?}",
                repair.qdrant_collection,
                repair.repaired,
                repair.points_upserted,
                repair.skipped_reason
            )
        })
        .unwrap_or_default();
    let sidecar_images_note = format!(
        "\n- sidecar_images: qdrant=`{}` embed=`{}`",
        payload.sidecar_state.sidecar_images.qdrant, payload.sidecar_state.sidecar_images.embed
    );
    let markdown = format!(
        "# Retrieval bootstrap\n\n- compose_started: {}\n- lexical_ready: {} ({})\n- qdrant_reachable: {} ({})\n- embed_reachable: {} ({})\n- embedding_device_policy: `{}` observed_device=`{}` observation_source=`{}` detected_provider={:?} detected_gpu={:?} accelerator_requested={} accelerator_request_provider={:?} accelerator_request_device={:?} cpu_allowed={}\n- retrieval_mode: `{}`\n- storage_repair: protected={} pruned={} invalid_dirs_removed={} stub_markers_migrated={} collections_seen={} overflow_protected={}{overflow_note}{scan_warning}{prune_suppressed_note}",
        payload.compose_started,
        payload.lexical_ready,
        payload.lexical_detail,
        payload.qdrant_reachable,
        payload.qdrant_detail,
        payload.embed_reachable,
        payload.embed_detail,
        report.infrastructure.embedding_device_policy,
        report.infrastructure.embedding_device_state,
        report.infrastructure.embedding_device_observation_source,
        report.infrastructure.embedding_detected_provider.as_deref(),
        report.infrastructure.embedding_detected_gpu.as_deref(),
        report.infrastructure.embedding_accelerator_requested,
        report
            .infrastructure
            .embedding_accelerator_request_provider
            .as_deref(),
        report
            .infrastructure
            .embedding_accelerator_request_device
            .as_deref(),
        report.infrastructure.embedding_cpu_allowed,
        payload.project_status.retrieval_mode,
        repair.protected_collections,
        repair.pruned_collections,
        repair.removed_invalid_dirs,
        repair.migrated_legacy_stub_markers,
        repair.collections_seen,
        repair.overflow_protected,
    );
    emit(
        format,
        &payload,
        format!("{markdown}{sidecar_images_note}{project_repair_note}"),
        output_file,
    )
}

fn emit_retrieval_status(
    format: OutputFormat,
    report: &RetrievalStatusReport,
    readiness_broker: &crate::readiness_broker::ReadinessBrokerSnapshot,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let mut payload = serde_json::to_value(report)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "readiness_broker".to_string(),
            serde_json::to_value(readiness_broker)?,
        );
    }
    let manifest_vector_backend = report
        .manifest_vector_embedding_backend
        .as_deref()
        .unwrap_or("<none>");
    let stored_doc_backend = report
        .stored_doc_vector_producer_backend
        .as_deref()
        .unwrap_or("<none>");
    let manifest_contract_note = report
        .manifest_contract
        .as_ref()
        .map(|contract| {
            let lanes = contract
                .lanes
                .iter()
                .map(|lane| format!("{}:{}:{}", lane.lane, lane.producer, lane.status))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "- manifest_contract: generation={:?} input_hash={:?} lanes=`{}`\n",
                contract.generation, contract.input_hash, lanes
            )
        })
        .unwrap_or_default();
    let repair_note = report
        .repair
        .as_ref()
        .map(|repair| {
            format!(
                "- repair: reason=`{}` next_step=\"{}\" next_command=`{}`\n",
                repair.reason, repair.next_step, repair.next_command
            )
        })
        .unwrap_or_default();
    let ownership_note = report
        .ownership
        .as_ref()
        .map(|ownership| {
            format!(
                "- ownership: owner=`{}` profile=`{}` namespace=`{}` cleanup=`{}` ports=qdrant:{} grpc:{} embed:{}\n",
                ownership.owner,
                ownership.profile,
                ownership.namespace,
                ownership.cleanup_command,
                ownership.ports.qdrant_http,
                ownership.ports.qdrant_grpc,
                ownership.ports.embed_http,
            )
        })
        .unwrap_or_default();
    let sidecar_images_note = format!(
        "- sidecar_images: qdrant=`{}` embed=`{}`\n",
        report.sidecar_images.qdrant, report.sidecar_images.embed
    );
    let broker_note = format!(
        "- readiness_broker: project_id={} persistence={} operations={} gpu_proof={}\n",
        readiness_broker.project_id,
        readiness_broker.persistence_status,
        readiness_broker.operations.len(),
        readiness_broker
            .gpu_proof
            .as_ref()
            .map(|proof| proof.proof_status.as_str())
            .unwrap_or("unknown")
    );
    let markdown = format!(
        "# Retrieval status\n\n- retrieval_mode: `{}`\n- degraded_reason: {:?}\n- query_embedding_backend: `{}`\n- embedding_device_policy: `{}` observed_device=`{}` observation_source=`{}` detected_provider={:?} detected_gpu={:?} accelerator_requested={} accelerator_request_provider={:?} accelerator_request_device={:?} cpu_allowed={}\n- manifest_vector_backend: `{}` dim={:?}\n- stored_doc_vector_producer: `{}` dim={:?} mixed_backends={:?}\n{}{}{}{}{}- lexical: {:?} ({:?}) capabilities: lexical={}\n- qdrant: {:?} ({:?}) capabilities: semantic={}\n- scip: {:?} ({:?}) capabilities: graph={}\n",
        report.retrieval_mode,
        report.degraded_reason,
        report.query_embedding_backend,
        report.embedding_device_policy,
        report.embedding_device_state,
        report.embedding_device_observation_source,
        report.embedding_detected_provider.as_deref(),
        report.embedding_detected_gpu.as_deref(),
        report.embedding_accelerator_requested,
        report.embedding_accelerator_request_provider.as_deref(),
        report.embedding_accelerator_request_device.as_deref(),
        report.embedding_cpu_allowed,
        manifest_vector_backend,
        report.manifest_vector_embedding_dim,
        stored_doc_backend,
        report.stored_doc_vector_dim,
        report.stored_doc_vector_mixed_backends,
        manifest_contract_note,
        repair_note,
        ownership_note,
        sidecar_images_note,
        broker_note,
        report.lexical.status,
        report.lexical.detail,
        report.lexical.capabilities.lexical,
        report.qdrant.status,
        report.qdrant.detail,
        report.qdrant.capabilities.semantic,
        report.scip.status,
        report.scip.detail,
        report.scip.capabilities.graph,
    );
    emit(format, &payload, markdown, output_file)
}

fn emit_retrieval_inventory(
    format: OutputFormat,
    report: &codestory_retrieval::SidecarInventoryReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let mut markdown = format!(
        "# Retrieval sidecar inventory\n\n- dry_run: {}\n- docker_available: {}\n- cache_root: `{}`\n",
        report.dry_run, report.docker_available, report.cache_root
    );
    if let Some(error) = report.docker_error.as_deref() {
        markdown.push_str(&format!("- docker_error: `{error}`\n"));
    }
    if let Some(retention) = report.generation_retention.as_ref() {
        markdown.push_str(&format!(
            "- generation_retention_active_bytes: {}\n- generation_retention_rollback_bytes: {}\n- generation_retention_building_bytes: {}\n- generation_retention_retained_bytes: {}\n- generation_retention_reclaimable_bytes: {}\n- generation_retention_pruning_suppressed: {}\n",
            retention.active_bytes,
            retention.rollback_bytes,
            retention.building_bytes,
            retention.retained_bytes,
            retention.reclaimable_bytes,
            retention.pruning_suppressed
        ));
        if !retention.errors.is_empty() {
            markdown.push_str(&format!(
                "- generation_retention_errors: `{}`\n",
                retention.errors.join("; ")
            ));
        }
    }
    if report.namespaces.is_empty() {
        markdown.push_str("\nNo sidecar namespaces found.\n");
    }
    for namespace in &report.namespaces {
        let ports = namespace
            .containers
            .iter()
            .filter_map(|container| container.ports.as_deref())
            .collect::<Vec<_>>()
            .join("; ");
        markdown.push_str(&format!(
            "\n## {}\n\n- state: `{:?}`\n- owner/profile: `{}` / `{}`\n- state_path: `{}`\n- cleanup_command: `{}`\n- age_ms: `{}`\n- compose_project: `{}`\n- containers: {}\n- networks: {}\n- ports: `{}`\n- model_dir: `{}` required_gguf=`{}` present={}\n",
            namespace.namespace,
            namespace.state,
            namespace.owner.as_deref().unwrap_or("<unknown>"),
            namespace.profile.as_deref().unwrap_or("<unknown>"),
            namespace.state_path,
            namespace.cleanup_command.as_deref().unwrap_or("<none>"),
            namespace
                .age_ms
                .map(|age| age.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            namespace.compose_project.as_deref().unwrap_or("<unknown>"),
            namespace.containers.len(),
            namespace.networks.len(),
            if ports.is_empty() { "<none>" } else { &ports },
            namespace.model.model_dir.as_deref().unwrap_or("<none>"),
            namespace.model.required_gguf,
            namespace.model.required_gguf_present,
        ));
        if !namespace.reasons.is_empty() {
            markdown.push_str(&format!("- reasons: `{}`\n", namespace.reasons.join("; ")));
        }
        if let Some(reason) = namespace.safe_candidate_reason.as_deref() {
            markdown.push_str(&format!("- safe_candidate_reason: `{reason}`\n"));
        }
        if let Some(reason) = namespace.blocking_reason.as_deref() {
            markdown.push_str(&format!("- blocking_reason: `{reason}`\n"));
        }
    }
    emit(format, report, markdown, output_file)
}

fn emit_retrieval_gc(
    format: OutputFormat,
    report: &codestory_retrieval::SidecarGcReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let mut markdown = format!(
        "# Retrieval sidecar GC\n\n- dry_run: {}\n- docker_available: {}\n- cache_root: `{}`\n- removed: {}\n- blocked: {}\n",
        report.dry_run,
        report.docker_available,
        report.cache_root,
        report.removed.len(),
        report.blocked.len()
    );
    if let Some(error) = report.docker_error.as_deref() {
        markdown.push_str(&format!("- docker_error: `{error}`\n"));
    }
    if let Some(retention) = report.generation_retention.as_ref() {
        markdown.push_str(&format!(
            "- generation_retention_active_bytes: {}\n- generation_retention_rollback_bytes: {}\n- generation_retention_building_bytes: {}\n- generation_retention_retained_bytes: {}\n- generation_retention_reclaimable_bytes: {}\n- generation_retention_removed_bytes: {}\n- generation_retention_remaining_reclaimable_bytes: {}\n- generation_retention_pruning_suppressed: {}\n",
            retention.active_bytes,
            retention.rollback_bytes,
            retention.building_bytes,
            retention.retained_bytes,
            retention.reclaimable_bytes,
            retention.removed_bytes,
            retention.remaining_reclaimable_bytes,
            retention.pruning_suppressed
        ));
        if !retention.errors.is_empty() {
            markdown.push_str(&format!(
                "- generation_retention_errors: `{}`\n",
                retention.errors.join("; ")
            ));
        }
    }
    markdown.push_str("\n## Removed namespaces\n");
    if report.removed.is_empty() {
        markdown.push_str("\nNone.\n");
    }
    for namespace in &report.removed {
        markdown.push_str(&format!(
            "\n- `{}` ({:?}): {}; paths={} docker_resources={}\n",
            namespace.namespace,
            namespace.state,
            namespace.reason,
            namespace.removed_paths.len(),
            namespace.removed_docker_resources.len()
        ));
    }
    markdown.push_str("\n## Blocked namespaces\n");
    if report.blocked.is_empty() {
        markdown.push_str("\nNone.\n");
    }
    for namespace in &report.blocked {
        markdown.push_str(&format!(
            "\n- `{}` ({:?}): {}",
            namespace.namespace, namespace.state, namespace.reason
        ));
        if !namespace.errors.is_empty() {
            markdown.push_str(&format!("; errors={}", namespace.errors.join("; ")));
        }
        markdown.push('\n');
    }
    emit(format, report, markdown, output_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::ffi::OsString;

    #[test]
    fn auto_refresh_retries_full_for_semantic_doc_contract_drift() {
        let error = anyhow!("sidecar_semantic_doc_embedding_contract_changed")
            .context("retrieval index finalize");

        assert!(retrieval_index_should_retry_full_refresh(
            RefreshMode::Auto,
            &error
        ));
        assert!(!retrieval_index_should_retry_full_refresh(
            RefreshMode::None,
            &error
        ));
        assert!(!retrieval_index_should_retry_full_refresh(
            RefreshMode::Incremental,
            &error
        ));
    }

    #[test]
    fn auto_refresh_does_not_retry_unrelated_finalize_errors() {
        let error = anyhow!("mandatory Qdrant semantic collection incomplete")
            .context("retrieval index finalize");

        assert!(!retrieval_index_should_retry_full_refresh(
            RefreshMode::Auto,
            &error
        ));
    }

    #[test]
    fn retrieval_index_embedding_policy_blocks_unknown_device_before_refresh() {
        let _lock = crate::config::config_env_test_lock();
        let _backend = EnvGuard::set("CODESTORY_EMBED_BACKEND", "llamacpp");
        let _real = EnvGuard::set("CODESTORY_RETRIEVAL_REAL_EMBEDDINGS", "1");
        let _allow_cpu = EnvGuard::remove("CODESTORY_EMBED_ALLOW_CPU");
        let _policy = EnvGuard::remove("CODESTORY_EMBED_DEVICE_POLICY");
        let _device = EnvGuard::remove("CODESTORY_EMBED_DEVICE_STATE");
        let sidecar = SidecarRuntimeConfig::local();

        let error = ensure_retrieval_index_embedding_policy(&sidecar)
            .expect_err("unknown embedding device must block retrieval index refresh");
        let message = format!("{error:#}");

        assert!(
            message.contains("retrieval index embedding device policy"),
            "error should preserve direct retrieval-index context: {error:#}"
        );
        assert!(
            message.contains("embedding_device_unverified"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn sidecar_runtime_retains_backend_endpoint_profile_and_run_without_env_mutation() {
        let _lock = crate::config::config_env_test_lock();
        let _runtime_mode = EnvGuard::remove("CODESTORY_EMBED_RUNTIME_MODE");
        let _backend = EnvGuard::remove("CODESTORY_EMBED_BACKEND");

        let project = tempfile::TempDir::new().expect("project");
        let sidecar = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some("packet-search-eval"),
        );
        let expected = codestory_retrieval::SidecarLayout::embed_base_url(sidecar.embed_http_port);

        assert_eq!(sidecar.embedding.backend, "llamacpp");
        assert_eq!(sidecar.embedding.endpoint, expected);
        assert_eq!(sidecar.profile, SidecarProfile::Agent);
        assert_eq!(sidecar.run_id.as_deref(), Some("packet-search-eval"));
        assert!(std::env::var("CODESTORY_EMBED_BACKEND").is_err());
        assert!(std::env::var("CODESTORY_EMBED_LLAMACPP_URL").is_err());
    }

    #[test]
    fn local_profile_handoff_reports_default_namespace_path_mismatch() {
        let project = tempfile::TempDir::new().expect("project");
        let local =
            codestory_retrieval::sidecar_runtime_for_project(project.path(), SidecarProfile::Local);
        let agent = codestory_retrieval::sidecar_runtime_for_project_with_run_id(
            project.path(),
            SidecarProfile::Agent,
            Some("issue-534"),
        );

        let message = sidecar_runtime_mismatch(&local, &agent).expect("mismatch");

        assert!(message.contains("retrieval profile handoff mismatch"));
        assert!(message.contains("profile=local"));
        assert!(message.contains("profile=agent"));
        assert!(message.contains("namespace=codestory-agent"));
        assert!(message.contains("lexical="));
        assert!(sidecar_runtime_mismatch(&local, &local).is_none());
    }

    struct EnvGuard {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, old }
        }

        fn remove(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = self.old.as_ref() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}
