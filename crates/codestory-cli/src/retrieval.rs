use anyhow::{Context, Result};
use codestory_contracts::api::IndexMode;
use std::time::Duration;

use codestory_retrieval::{
    BootstrapStorageScope, FinalizeIndexOutcome, ProjectQdrantRepairOutcome, QueryRequest,
    RetrievalIndexManifest, RetrievalStatusReport, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED,
    bootstrap_sidecars, execute_retrieval_query, sidecar_down, sidecar_up, strict_sidecar_status,
};

use crate::args::{
    OutputFormat, RefreshMode, RetrievalAction, RetrievalBootstrapCommand, RetrievalCommand,
    RetrievalIndexCommand, RetrievalQueryCommand, RetrievalStatusCommand,
};
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error, resolve_refresh_request};

pub(crate) fn run_retrieval(cmd: RetrievalCommand) -> Result<()> {
    match cmd.action {
        RetrievalAction::Bootstrap(bootstrap_cmd) => run_retrieval_bootstrap(bootstrap_cmd),
        RetrievalAction::Up => run_retrieval_up(),
        RetrievalAction::Down => run_retrieval_down(),
        RetrievalAction::Status(status_cmd) => run_retrieval_status(status_cmd),
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
    let report = bootstrap_sidecars(
        Some(&runtime.project_root),
        &storage_scope,
        cmd.compose_file.as_deref(),
        cmd.skip_compose,
        Duration::from_secs(cmd.wait_secs),
    )
    .context("retrieval bootstrap")?;
    let project_qdrant_repair = codestory_retrieval::repair_project_qdrant_collection(
        &runtime.project_root,
        &runtime.storage_path,
    )
    .context("retrieval project qdrant repair")?;
    let status = strict_sidecar_status(&runtime.project_root, Some(&runtime.storage_path))
        .context("retrieval status after bootstrap")?;
    emit_retrieval_bootstrap(
        cmd.format,
        &report,
        project_qdrant_repair.as_ref(),
        &status,
        cmd.output_file.as_deref(),
    )
}

fn run_retrieval_up() -> Result<()> {
    let state = sidecar_up().context("retrieval up")?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

fn run_retrieval_down() -> Result<()> {
    sidecar_down().context("retrieval down")?;
    println!("retrieval sidecar state cleared");
    Ok(())
}

fn run_retrieval_status(cmd: RetrievalStatusCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let report = strict_sidecar_status(&runtime.project_root, Some(&runtime.storage_path))
        .context("retrieval status")?;
    emit_retrieval_status(cmd.format, &report, cmd.output_file.as_deref())
}

fn run_retrieval_query(cmd: RetrievalQueryCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let result = execute_retrieval_query(QueryRequest {
        project_root: &runtime.project_root,
        storage_path: &runtime.storage_path,
        query: &cmd.query,
        budget_ms: cmd.budget_ms,
        cancelled: None,
    })
    .context("retrieval query")?;
    emit_retrieval_query(cmd.format, &result, cmd.output_file.as_deref())
}

fn run_retrieval_index(cmd: RetrievalIndexCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let summary = runtime.open_project_summary()?;
    let refresh_mode = resolve_refresh_request(cmd.refresh, &summary);
    run_retrieval_index_refresh(&runtime, cmd.refresh, refresh_mode)?;
    let outcome = finalize_retrieval_index_for_runtime(&runtime).or_else(|error| {
        if !retrieval_index_should_retry_full_refresh(cmd.refresh, &error) {
            return Err(error);
        }
        runtime
            .index
            .run_indexing_blocking(IndexMode::Full)
            .map_err(map_api_error)?;
        finalize_retrieval_index_for_runtime(&runtime)
            .context("retrieval index finalize after semantic-doc contract repair")
    })?;
    emit_retrieval_index(cmd.format, &outcome, cmd.output_file.as_deref())
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
    let opened = runtime.ensure_open(crate::args::RefreshMode::None)?;
    ensure_index_ready(&opened, "retrieval index")?;
    codestory_retrieval::finalize_index(&runtime.project_root, &runtime.storage_path)
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
    zoekt_stubbed: bool,
    qdrant_stubbed: bool,
    scip_stubbed: bool,
}

fn emit_retrieval_index(
    format: OutputFormat,
    outcome: &FinalizeIndexOutcome,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let payload = RetrievalIndexOutput {
        manifest: &outcome.manifest,
        degraded_modes: &outcome.degraded_modes,
        zoekt_stubbed: outcome.zoekt_stubbed,
        qdrant_stubbed: outcome.qdrant_stubbed,
        scip_stubbed: outcome.scip_stubbed,
    };
    let markdown = format!(
        "# Retrieval index\n\n- project_id: `{}`\n- zoekt_version: `{}`\n- qdrant_collection: `{}`\n- scip_revision: {:?}\n- degraded_modes: {:?}\n",
        payload.manifest.project_id,
        payload.manifest.zoekt_version,
        payload.manifest.qdrant_collection,
        payload.manifest.scip_revision,
        payload.degraded_modes,
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
    zoekt_reachable: bool,
    qdrant_reachable: bool,
    embed_reachable: bool,
    zoekt_detail: &'a str,
    qdrant_detail: &'a str,
    embed_detail: &'a str,
    storage_repair: &'a codestory_retrieval::QdrantStorageRepairReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_qdrant_repair: Option<&'a ProjectQdrantRepairOutcome>,
    sidecar_state: &'a codestory_retrieval::SidecarStateFile,
    project_status: &'a RetrievalStatusReport,
}

fn emit_retrieval_bootstrap(
    format: OutputFormat,
    report: &codestory_retrieval::BootstrapReport,
    project_qdrant_repair: Option<&ProjectQdrantRepairOutcome>,
    status: &RetrievalStatusReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let compose_path = report
        .compose_file
        .as_ref()
        .map(|path| path.display().to_string());
    let payload = RetrievalBootstrapOutput {
        compose_started: report.compose_started,
        compose_file: compose_path.as_deref(),
        zoekt_reachable: report.infrastructure.zoekt_reachable,
        qdrant_reachable: report.infrastructure.qdrant_reachable,
        embed_reachable: report.infrastructure.embed_reachable,
        zoekt_detail: &report.infrastructure.zoekt_detail,
        qdrant_detail: &report.infrastructure.qdrant_detail,
        embed_detail: &report.infrastructure.embed_detail,
        storage_repair: &report.storage_repair,
        project_qdrant_repair,
        sidecar_state: &report.state,
        project_status: status,
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
    let markdown = format!(
        "# Retrieval bootstrap\n\n- compose_started: {}\n- zoekt_reachable: {} ({})\n- qdrant_reachable: {} ({})\n- embed_reachable: {} ({})\n- retrieval_mode: `{}`\n- storage_repair: protected={} pruned={} invalid_dirs_removed={} stub_markers_migrated={} collections_seen={} overflow_protected={}{overflow_note}{scan_warning}{prune_suppressed_note}",
        payload.compose_started,
        payload.zoekt_reachable,
        payload.zoekt_detail,
        payload.qdrant_reachable,
        payload.qdrant_detail,
        payload.embed_reachable,
        payload.embed_detail,
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
        format!("{markdown}{project_repair_note}"),
        output_file,
    )
}

fn emit_retrieval_status(
    format: OutputFormat,
    report: &RetrievalStatusReport,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let manifest_vector_backend = report
        .manifest_vector_embedding_backend
        .as_deref()
        .unwrap_or("<none>");
    let stored_doc_backend = report
        .stored_doc_vector_producer_backend
        .as_deref()
        .unwrap_or("<none>");
    let markdown = format!(
        "# Retrieval status\n\n- retrieval_mode: `{}`\n- degraded_reason: {:?}\n- query_embedding_backend: `{}`\n- manifest_vector_backend: `{}` dim={:?}\n- stored_doc_vector_producer: `{}` dim={:?} mixed_backends={:?}\n- zoekt: {:?} ({:?}) capabilities: lexical={}\n- qdrant: {:?} ({:?}) capabilities: semantic={}\n- scip: {:?} ({:?}) capabilities: graph={}\n",
        report.retrieval_mode,
        report.degraded_reason,
        report.query_embedding_backend,
        manifest_vector_backend,
        report.manifest_vector_embedding_dim,
        stored_doc_backend,
        report.stored_doc_vector_dim,
        report.stored_doc_vector_mixed_backends,
        report.zoekt.status,
        report.zoekt.detail,
        report.zoekt.capabilities.lexical,
        report.qdrant.status,
        report.qdrant.detail,
        report.qdrant.capabilities.semantic,
        report.scip.status,
        report.scip.detail,
        report.scip.capabilities.graph,
    );
    emit(format, report, markdown, output_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

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
}
