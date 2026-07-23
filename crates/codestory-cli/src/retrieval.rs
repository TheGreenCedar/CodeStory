use anyhow::{Context, Result, bail};
use codestory_contracts::api::IndexMode;
use std::sync::atomic::{AtomicBool, Ordering};

use codestory_retrieval::{
    FinalizeIndexOutcome, QueryRequest, RetrievalIndexManifest, RetrievalStatusReport,
    SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED, SidecarRuntimeConfig, strict_sidecar_status_for_runtime,
};

use crate::args::{
    CliSidecarProfile, OutputFormat, RefreshMode, RetrievalAction, RetrievalCommand,
    RetrievalIndexCommand, RetrievalInventoryCommand, RetrievalQueryCommand,
    RetrievalRepublishProjectionsCommand, RetrievalStatusCommand,
};
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, annotate_refresh_error, ensure_index_ready, map_api_error};

pub(crate) fn run_retrieval(cmd: RetrievalCommand) -> Result<()> {
    match cmd.action {
        RetrievalAction::Status(status_cmd) => run_retrieval_status(status_cmd),
        RetrievalAction::Inventory(inventory_cmd) => run_retrieval_inventory(inventory_cmd),
        RetrievalAction::Index(index_cmd) => run_retrieval_index(index_cmd),
        RetrievalAction::RepublishProjections(republish_cmd) => {
            run_retrieval_republish_projections(republish_cmd)
        }
        RetrievalAction::Query(query_cmd) => run_retrieval_query(query_cmd),
    }
}

fn run_retrieval_republish_projections(cmd: RetrievalRepublishProjectionsCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let outcome = runtime
        .index
        .republish_semantic_projections_at_blocking(
            runtime.project_root.clone(),
            runtime.storage_path.clone(),
        )
        .map_err(map_api_error)?;
    let markdown = format!(
        "# Semantic projection republish\n\n- previous_generation: `{}`\n- generation: `{}`\n- generation_id: `{}`\n- semantic_policy_version: `{}`\n- symbol_documents: {}\n- dense_anchors: {}\n",
        outcome.previous_publication.generation,
        outcome.publication.generation,
        outcome.publication.generation_id,
        outcome.semantic_policy_version,
        outcome.symbol_document_count,
        outcome.dense_anchor_count,
    );
    emit(cmd.format, &outcome, markdown, cmd.output_file.as_deref())
}

pub(crate) fn run_retrieval_status(cmd: RetrievalStatusCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let profile = cmd
        .profile
        .or_else(|| cmd.run_id.as_ref().map(|_| CliSidecarProfile::Agent));
    let report = if let Some(profile) = profile {
        let sidecar = runtime.sidecar.with_profile_and_run_id(
            Some(&runtime.project_root),
            profile.into(),
            cmd.run_id.as_deref(),
        );
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
    emit_retrieval_status(cmd.format, &report, cmd.output_file.as_deref())
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
    let decision = runtime.resolve_refresh_decision_with_preflight(cmd.refresh)?;
    let refresh_mode = decision.effective_mode;
    ensure_retrieval_index_embedding_policy(&sidecar)?;
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
    runtime.open_project_summary()?;
    runtime
        .index
        .run_indexing_blocking(mode)
        .map_err(|error| map_api_error(annotate_refresh_error(error, requested_refresh, mode)))
        .map(|_| ())
        .or_else(|error| {
            if !retrieval_index_should_retry_full_refresh(requested_refresh, &error) {
                return Err(error);
            }
            runtime
                .index
                .run_indexing_blocking(IndexMode::Full)
                .map_err(|error| {
                    map_api_error(annotate_refresh_error(
                        error,
                        requested_refresh,
                        IndexMode::Full,
                    ))
                })
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
    let cancelled = AtomicBool::new(false);
    finalize_retrieval_index_for_sidecar_runtime_with_cancel(runtime, sidecar, &cancelled)
}

pub(crate) fn finalize_retrieval_index_for_sidecar_runtime_with_cancel(
    runtime: &RuntimeContext,
    sidecar: &SidecarRuntimeConfig,
    cancelled: &AtomicBool,
) -> Result<FinalizeIndexOutcome> {
    if cancelled.load(Ordering::Acquire) {
        bail!("retrieval index cancelled before opening the project runtime");
    }
    let opened = runtime.ensure_open(crate::args::RefreshMode::None)?;
    ensure_index_ready(&opened, "retrieval index")?;
    codestory_retrieval::finalize_index_for_runtime_with_cancel(
        &runtime.project_root,
        &runtime.storage_path,
        sidecar,
        cancelled,
    )
    .context("retrieval index finalize")
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
        scip_stubbed: outcome.scip_stubbed,
        generation_retention_plan: &outcome.generation_retention_plan,
        generation_retention: &outcome.generation_retention,
    };
    let markdown = format!(
        "# Retrieval index\n\n- project_id: `{}`\n- lexical_version: `{}`\n- semantic_generation: `{}`\n- scip_revision: {:?}\n- degraded_modes: {:?}\n- retention_retained_bytes: {}\n- retention_reclaimable_bytes: {}\n- retention_removed_bytes: {}\n- retention_remaining_reclaimable_bytes: {}\n- retention_pruning_suppressed: {}\n",
        payload.manifest.project_id,
        payload.manifest.lexical_version,
        payload.manifest.semantic_generation,
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

fn emit_retrieval_status(
    format: OutputFormat,
    report: &RetrievalStatusReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let manifest_vector_embedding_backend = report
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
    let markdown = format!(
        "# Retrieval status\n\n- retrieval_mode: `{}`\n- degraded_reason: {:?}\n- query_embedding_backend: `{}`\n- embedding_device_policy: `{}` observed_device=`{}` observation_source=`{}` detected_provider={:?} detected_gpu={:?} accelerator_requested={} accelerator_request_provider={:?} accelerator_request_device={:?} cpu_allowed={}\n- manifest_vector_embedding_backend: `{}` dim={:?}\n- stored_doc_vector_producer: `{}` dim={:?} mixed_backends={:?}\n{}- lexical: {:?} ({:?}) capabilities: lexical={}\n- semantic: {:?} ({:?}) capabilities: semantic={}\n- scip: {:?} ({:?}) capabilities: graph={}\n",
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
        manifest_vector_embedding_backend,
        report.manifest_vector_embedding_dim,
        stored_doc_backend,
        report.stored_doc_vector_dim,
        report.stored_doc_vector_mixed_backends,
        manifest_contract_note,
        report.lexical.status,
        report.lexical.detail,
        report.lexical.capabilities.lexical,
        report.semantic.status,
        report.semantic.detail,
        report.semantic.capabilities.semantic,
        report.scip.status,
        report.scip.detail,
        report.scip.capabilities.graph,
    );
    emit(format, report, markdown, output_file)
}

fn emit_retrieval_inventory(
    format: OutputFormat,
    report: &codestory_retrieval::SidecarInventoryReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let mut markdown = format!(
        "# Retrieval runtime inventory\n\n- dry_run: {}\n- cache_root: `{}`\n",
        report.dry_run, report.cache_root
    );
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
    emit(format, report, markdown, output_file)
}

fn emit_retrieval_gc(
    format: OutputFormat,
    report: &codestory_retrieval::SidecarGcReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let mut markdown = format!(
        "# Retrieval runtime GC\n\n- dry_run: {}\n- cache_root: `{}`\n",
        report.dry_run, report.cache_root,
    );
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
    emit(format, report, markdown, output_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(windows))]
    use crate::args::ProjectArgs;
    use anyhow::anyhow;
    #[cfg(not(windows))]
    use std::fs;
    #[cfg(not(windows))]
    use tempfile::tempdir;

    #[cfg(not(windows))]
    #[test]
    fn compatible_auto_refresh_opens_the_project_before_indexing() {
        let temp = tempdir().expect("temporary test root");
        let project = temp.path().join("project");
        let cache = temp.path().join("cache");
        fs::create_dir_all(project.join("src")).expect("create source directory");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"retrieval-refresh-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write manifest");
        let source = project.join("src/lib.rs");
        fs::write(&source, "pub fn before() {}\n").expect("write source");
        let args = ProjectArgs {
            project: project.clone(),
            cache_dir: Some(cache),
        };

        let seed = RuntimeContext::new_inspect_only(&args).expect("seed runtime");
        seed.ensure_open(RefreshMode::Full)
            .expect("publish compatible core");
        fs::write(&source, "pub fn after() {}\n").expect("change source");

        let runtime = RuntimeContext::new_inspect_only(&args).expect("retrieval runtime");
        let decision = runtime
            .resolve_refresh_decision_with_preflight(RefreshMode::Auto)
            .expect("resolve compatible auto refresh");
        assert_eq!(decision.effective_mode, Some(IndexMode::Incremental));
        run_retrieval_index_refresh(&runtime, RefreshMode::Auto, decision.effective_mode)
            .expect("run compatible incremental refresh");
    }

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
        let error =
            anyhow!("mandatory semantic generation incomplete").context("retrieval index finalize");

        assert!(!retrieval_index_should_retry_full_refresh(
            RefreshMode::Auto,
            &error
        ));
    }
}
