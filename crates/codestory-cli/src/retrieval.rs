use anyhow::{Context, Result, bail};
use codestory_contracts::api::{
    IncrementalCoreWallTimings, IncrementalScheduledPathDto, IndexMode, IndexingPhaseTimings,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use codestory_runtime::{
    FinalizeComponentWork, FinalizeIndexOutcome, FinalizePhaseTiming, RetrievalIndexManifest,
    RetrievalStatusReport, RuntimeRetrievalConfig, SIDECAR_SEMANTIC_DOC_CONTRACT_CHANGED,
};

use crate::args::{
    CliSidecarProfile, OutputFormat, RefreshMode, RetrievalAction,
    RetrievalActivateRollbackCommand, RetrievalActivateRollbackOutput, RetrievalCommand,
    RetrievalIndexCommand, RetrievalInventoryCommand, RetrievalQueryCommand,
    RetrievalRepublishProjectionsCommand, RetrievalStatusCommand,
};
use crate::output::{emit, validate_output_file_parent};
use crate::runtime::{RuntimeContext, annotate_refresh_error, ensure_index_ready, map_api_error};

#[derive(serde::Serialize)]
struct ObservedRetrievalStatus<'a> {
    #[serde(flatten)]
    report: &'a RetrievalStatusReport,
    #[serde(flatten)]
    ready_lease: &'a codestory_runtime::ReadyLeaseEvidence,
}

pub(crate) fn run_retrieval(cmd: RetrievalCommand) -> Result<()> {
    match cmd.action {
        RetrievalAction::Status(status_cmd) => run_retrieval_status(status_cmd),
        RetrievalAction::Inventory(inventory_cmd) => run_retrieval_inventory(inventory_cmd),
        RetrievalAction::Index(index_cmd) => run_retrieval_index(index_cmd),
        RetrievalAction::RepublishProjections(republish_cmd) => {
            run_retrieval_republish_projections(republish_cmd)
        }
        RetrievalAction::Query(query_cmd) => run_retrieval_query(query_cmd),
        RetrievalAction::ActivateRollback(activate_cmd) => {
            run_retrieval_activate_rollback(activate_cmd)
        }
    }
}

fn run_retrieval_activate_rollback(cmd: RetrievalActivateRollbackCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let outcome = runtime
        .activation
        .activate_retained_rollback_generation(
            &runtime.project_root,
            &runtime.storage_path,
            !cmd.dry_run,
        )
        .map_err(annotate_rollback_activation_error)?;
    let project = crate::display::clean_path_string(&runtime.project_root.to_string_lossy());
    let next_commands = rollback_activation_next_commands(&project, &outcome);
    let markdown = render_rollback_activation_markdown(&project, &outcome, &next_commands);
    let payload = RetrievalActivateRollbackOutput {
        project,
        outcome,
        next_commands,
    };
    emit(cmd.format, &payload, markdown, cmd.output_file.as_deref())
}

/// Carry the typed refusal code into the CLI error chain.
///
/// The refusal is the product answer, not an internal failure, so the code has
/// to survive into stderr and into any wrapping context a caller adds.
fn annotate_rollback_activation_error(
    error: codestory_runtime::RollbackActivationError,
) -> anyhow::Error {
    let code = error.code();
    anyhow::anyhow!("{error}").context(format!("retrieval activate-rollback refused: {code}"))
}

fn rollback_activation_next_commands(
    project: &str,
    outcome: &codestory_runtime::RollbackActivationOutcome,
) -> Vec<String> {
    if outcome.applied {
        return vec![
            format!("codestory-cli doctor --project \"{project}\" --format markdown"),
            format!("codestory-cli retrieval inventory --project \"{project}\" --apply"),
        ];
    }
    vec![format!(
        "codestory-cli retrieval activate-rollback --project \"{project}\""
    )]
}

fn render_rollback_activation_markdown(
    project: &str,
    outcome: &codestory_runtime::RollbackActivationOutcome,
    next_commands: &[String],
) -> String {
    let mut markdown = format!(
        "# Retrieval rollback activation\n\n- project: `{project}`\n- project_id: `{}`\n- applied: {}\n- previous_generation: `{}`\n- activated_generation: `{}`\n- activated_semantic_generation: `{}`\n- activated_retrieval_mode: `{}`\n- rollback_pointer_retained: {}\n",
        outcome.project_id,
        outcome.applied,
        outcome
            .previous_generation
            .as_deref()
            .unwrap_or("<missing>"),
        outcome.activated_generation,
        outcome.activated_semantic_generation,
        outcome.activated_retrieval_mode,
        outcome.rollback_pointer_retained,
    );
    if !outcome.applied {
        markdown.push_str(
            "\nValidation only: the current retrieval generation was not changed. Rerun without `--dry-run` to activate.\n",
        );
    }
    markdown.push_str("\n## Next\n\n");
    for command in next_commands {
        markdown.push_str(&format!("- `{command}`\n"));
    }
    markdown
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
    let observation = if let Some(profile) = profile {
        runtime.activation.retrieval_status_for_profile(
            &runtime.project_root,
            &runtime.storage_path,
            profile.into(),
            cmd.run_id.as_deref(),
        )
    } else {
        runtime
            .activation
            .retrieval_status(&runtime.project_root, &runtime.storage_path)
    };
    let (report, ready_lease) = retrieval_status_result(observation)?;
    emit_retrieval_status(
        cmd.format,
        &report,
        &ready_lease,
        cmd.output_file.as_deref(),
    )
}

fn retrieval_status_result(
    result: std::result::Result<
        codestory_runtime::RetrievalStatusObservation,
        codestory_runtime::RetrievalStatusObservationError,
    >,
) -> anyhow::Result<(RetrievalStatusReport, codestory_runtime::ReadyLeaseEvidence)> {
    match result {
        Ok(observation) => {
            let ready_lease = observation.ready_lease().clone();
            Ok((observation.into_parts().1, ready_lease))
        }
        Err(error) => retrieval_status_error(error.into_parts().1),
    }
}

fn retrieval_status_error<T>(error: anyhow::Error) -> anyhow::Result<T> {
    Err(error).context("retrieval status")
}

pub(crate) fn run_retrieval_inventory(cmd: RetrievalInventoryCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    if cmd.apply {
        let report = runtime
            .activation
            .apply_retrieval_gc(&runtime.project_root, &runtime.storage_path)
            .context("retrieval inventory apply")?;
        return emit_retrieval_gc(cmd.format, &report, cmd.output_file.as_deref());
    }
    let report = runtime
        .activation
        .retrieval_inventory(&runtime.project_root, &runtime.storage_path)
        .context("retrieval inventory")?;
    emit_retrieval_inventory(cmd.format, &report, cmd.output_file.as_deref())
}

fn run_retrieval_query(cmd: RetrievalQueryCommand) -> Result<()> {
    preflight_output(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let result = runtime
        .activation
        .execute_retrieval_query(
            &runtime.project_root,
            &runtime.storage_path,
            &cmd.query,
            cmd.budget_ms,
        )
        .context("retrieval query")?;
    emit_retrieval_query(cmd.format, &result, cmd.output_file.as_deref())
}

fn run_retrieval_index(cmd: RetrievalIndexCommand) -> Result<()> {
    let command_started = Instant::now();
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
    let mut core_phase_timings = run_retrieval_index_refresh(&runtime, cmd.refresh, refresh_mode)?;
    let retrieval_started = Instant::now();
    let outcome = match finalize_retrieval_index_for_sidecar_runtime(&runtime, &sidecar) {
        Ok(outcome) => outcome,
        Err(error) if retrieval_index_should_retry_full_refresh(cmd.refresh, &error) => {
            core_phase_timings = Some(
                runtime
                    .index
                    .run_indexing_blocking(IndexMode::Full)
                    .map_err(map_api_error)?,
            );
            finalize_retrieval_index_for_sidecar_runtime(&runtime, &sidecar)
                .context("retrieval index finalize after semantic-doc contract repair")?
        }
        Err(error) => return Err(error),
    };
    let retrieval_finalize_ms = retrieval_started
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let incremental_wall_receipt = match (
        refresh_mode,
        core_phase_timings
            .as_ref()
            .and_then(|timings| timings.incremental_core_wall.as_ref()),
    ) {
        (Some(IndexMode::Incremental), Some(core_wall)) => Some(
            build_incremental_wall_receipt(
                command_started
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                retrieval_finalize_ms,
                core_wall,
                &outcome.phase_timings,
            )
            .context("incremental wall receipt")?,
        ),
        _ => None,
    };
    emit_retrieval_index(
        cmd.format,
        &outcome,
        core_phase_timings.as_ref(),
        incremental_wall_receipt.as_ref(),
        cmd.output_file.as_deref(),
    )
}

fn ensure_retrieval_index_embedding_policy(sidecar: &RuntimeRetrievalConfig) -> Result<()> {
    codestory_runtime::ensure_product_embedding_backend_for_runtime(sidecar)
        .context("retrieval index embedding device policy")
}

#[cfg(test)]
mod embedding_preflight_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn retrieval_index_embedding_preflight_preserves_cli_error_text() {
        let fixture = tempdir().expect("embedding preflight fixture");
        let cache_root = fixture.path().join("unavailable");
        std::fs::create_dir_all(&cache_root).expect("create embedding cache root");
        std::fs::write(
            cache_root.join(codestory_retrieval::TEST_EMBEDDING_UNAVAILABLE_MARKER),
            b"unavailable",
        )
        .expect("write embedding unavailable marker");
        let sidecar = codestory_retrieval::with_test_cache_root(&cache_root, || {
            crate::sidecar_runtime::for_project_with_run_id(
                fixture.path(),
                codestory_runtime::RuntimeRetrievalProfile::Agent,
                Some("preflight-run"),
            )
        });

        let error = ensure_retrieval_index_embedding_policy(&sidecar)
            .expect_err("unavailable embedding backend must block retrieval index");
        assert_eq!(
            format!("{error:#}"),
            format!(
                "retrieval index embedding device policy: embedding backend unavailable by test marker in {}",
                cache_root.display()
            )
        );
    }
}

fn run_retrieval_index_refresh(
    runtime: &RuntimeContext,
    requested_refresh: RefreshMode,
    refresh_mode: Option<IndexMode>,
) -> Result<Option<IndexingPhaseTimings>> {
    let Some(mode) = refresh_mode else {
        return Ok(None);
    };
    runtime.open_project_summary()?;
    match runtime
        .index
        .run_indexing_blocking(mode)
        .map_err(|error| map_api_error(annotate_refresh_error(error, requested_refresh, mode)))
    {
        Ok(timings) => Ok(Some(timings)),
        Err(error) if retrieval_index_should_retry_full_refresh(requested_refresh, &error) => {
            let timings = runtime
                .index
                .run_indexing_blocking(IndexMode::Full)
                .map_err(|error| {
                    map_api_error(annotate_refresh_error(
                        error,
                        requested_refresh,
                        IndexMode::Full,
                    ))
                })
                .context("retrieval index full refresh after semantic-doc contract repair")?;
            Ok(Some(timings))
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn finalize_retrieval_index_for_runtime(
    runtime: &RuntimeContext,
) -> Result<FinalizeIndexOutcome> {
    finalize_retrieval_index_for_sidecar_runtime(runtime, &runtime.sidecar)
}

pub(crate) fn finalize_retrieval_index_for_sidecar_runtime(
    runtime: &RuntimeContext,
    sidecar: &RuntimeRetrievalConfig,
) -> Result<FinalizeIndexOutcome> {
    let cancelled = AtomicBool::new(false);
    finalize_retrieval_index_for_sidecar_runtime_with_cancel(runtime, sidecar, &cancelled)
}

pub(crate) fn finalize_retrieval_index_for_sidecar_runtime_with_cancel(
    runtime: &RuntimeContext,
    sidecar: &RuntimeRetrievalConfig,
    cancelled: &AtomicBool,
) -> Result<FinalizeIndexOutcome> {
    if cancelled.load(Ordering::Acquire) {
        bail!("retrieval index cancelled before opening the project runtime");
    }
    let opened = runtime.ensure_open(crate::args::RefreshMode::None)?;
    ensure_index_ready(&opened, "retrieval index")?;
    runtime
        .activation
        .finalize_retrieval_index_with_cancel(
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
    generation_retention_plan: &'a codestory_runtime::GenerationRetentionPlan,
    generation_retention: &'a codestory_runtime::GenerationRetentionApplyReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    core_phase_timings: Option<&'a IndexingPhaseTimings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incremental_wall_receipt: Option<&'a IncrementalWallReceiptV1>,
    retrieval_phase_timings: Vec<RetrievalPhaseTimingOutput<'a>>,
    retrieval_component_work: Vec<RetrievalComponentWorkOutput<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct IncrementalWallReceiptV1 {
    contract: &'static str,
    total_ms: u64,
    core_refresh_ms: u64,
    retrieval_finalize_ms: u64,
    accounted_ms: u64,
    reconciliation_basis_points: u32,
    attributed_ms: u64,
    attributed_basis_points: u32,
    phases: IncrementalWallReceiptPhasesV1,
    scheduled_paths: Vec<IncrementalScheduledPathDto>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
struct IncrementalWallReceiptPhasesV1 {
    discovery_and_scheduling_ms: u64,
    stage_open_ms: u64,
    parse_and_extraction_ms: u64,
    core_staging_and_mutation_ms: u64,
    candidate_sealing_ms: u64,
    pointer_publication_ms: u64,
    lexical_ms: u64,
    vector_ms: u64,
    graph_ms: u64,
    retrieval_input_fingerprint_ms: u64,
    retrieval_manifest_publication_ms: u64,
    lock_wait_ms: u64,
    process_ipc_ms: u64,
    unattributed_ms: u64,
}

impl IncrementalWallReceiptPhasesV1 {
    fn sum(&self) -> u64 {
        self.discovery_and_scheduling_ms
            .saturating_add(self.stage_open_ms)
            .saturating_add(self.parse_and_extraction_ms)
            .saturating_add(self.core_staging_and_mutation_ms)
            .saturating_add(self.candidate_sealing_ms)
            .saturating_add(self.pointer_publication_ms)
            .saturating_add(self.lexical_ms)
            .saturating_add(self.vector_ms)
            .saturating_add(self.graph_ms)
            .saturating_add(self.retrieval_input_fingerprint_ms)
            .saturating_add(self.retrieval_manifest_publication_ms)
            .saturating_add(self.lock_wait_ms)
            .saturating_add(self.process_ipc_ms)
            .saturating_add(self.unattributed_ms)
    }
}

fn phase_total(timings: &[FinalizePhaseTiming], phase: &str) -> u64 {
    timings
        .iter()
        .filter(|timing| timing.phase == phase)
        .fold(0_u64, |total, timing| {
            total.saturating_add(timing.elapsed_ms)
        })
}

fn basis_points(numerator: u64, denominator: u64) -> u32 {
    if denominator == 0 {
        return 10_000;
    }
    numerator
        .saturating_mul(10_000)
        .checked_div(denominator)
        .unwrap_or_default()
        .min(10_000) as u32
}

fn build_incremental_wall_receipt(
    total_ms: u64,
    retrieval_finalize_ms: u64,
    core: &IncrementalCoreWallTimings,
    retrieval: &[FinalizePhaseTiming],
) -> Result<IncrementalWallReceiptV1> {
    let core_named_ms = u64::from(core.discovery_and_scheduling_ms)
        .saturating_add(u64::from(core.stage_open_ms))
        .saturating_add(u64::from(core.parse_and_extraction_ms))
        .saturating_add(u64::from(core.core_staging_and_mutation_ms))
        .saturating_add(u64::from(core.candidate_sealing_ms))
        .saturating_add(u64::from(core.pointer_publication_ms))
        .saturating_add(u64::from(core.lock_wait_ms))
        .saturating_add(u64::from(core.unattributed_ms));
    if core_named_ms != u64::from(core.core_refresh_ms) {
        bail!(
            "incremental core wall does not reconcile: phases={core_named_ms} total={}",
            core.core_refresh_ms
        );
    }
    let vector_complete = phase_total(retrieval, "embedded vectors");
    let vector_incremental = phase_total(retrieval, "incremental embedded vectors");
    if vector_complete > 0 && vector_incremental > 0 {
        bail!("retrieval finalization reported both complete and incremental vector totals");
    }
    let vector_total = vector_complete.max(vector_incremental);
    let process_ipc_ms = phase_total(retrieval, "vector ipc and orchestration").min(vector_total);
    let retrieval_input_fingerprint_ms = phase_total(retrieval, "input fingerprint");
    let lexical_ms = phase_total(retrieval, "lexical sidecar");
    let graph_ms = phase_total(retrieval, "graph artifact");
    let retrieval_manifest_publication_ms = phase_total(retrieval, "manifest write");
    let retrieval_named_ms = retrieval_input_fingerprint_ms
        .saturating_add(lexical_ms)
        .saturating_add(vector_total)
        .saturating_add(graph_ms)
        .saturating_add(retrieval_manifest_publication_ms);
    if retrieval_named_ms > retrieval_finalize_ms {
        bail!(
            "retrieval phases exceed finalization wall: phases={retrieval_named_ms} total={retrieval_finalize_ms}"
        );
    }
    let command_named_ms = u64::from(core.core_refresh_ms).saturating_add(retrieval_finalize_ms);
    if command_named_ms > total_ms {
        bail!(
            "core and retrieval phases exceed command wall: phases={command_named_ms} total={total_ms}"
        );
    }
    let unattributed_ms = u64::from(core.unattributed_ms)
        .saturating_add(retrieval_finalize_ms.saturating_sub(retrieval_named_ms))
        .saturating_add(total_ms.saturating_sub(command_named_ms));
    let phases = IncrementalWallReceiptPhasesV1 {
        discovery_and_scheduling_ms: u64::from(core.discovery_and_scheduling_ms),
        stage_open_ms: u64::from(core.stage_open_ms),
        parse_and_extraction_ms: u64::from(core.parse_and_extraction_ms),
        core_staging_and_mutation_ms: u64::from(core.core_staging_and_mutation_ms),
        candidate_sealing_ms: u64::from(core.candidate_sealing_ms),
        pointer_publication_ms: u64::from(core.pointer_publication_ms),
        lexical_ms,
        vector_ms: vector_total.saturating_sub(process_ipc_ms),
        graph_ms,
        retrieval_input_fingerprint_ms,
        retrieval_manifest_publication_ms,
        lock_wait_ms: u64::from(core.lock_wait_ms),
        process_ipc_ms,
        unattributed_ms,
    };
    let accounted_ms = phases.sum();
    let attributed_ms = total_ms.saturating_sub(unattributed_ms);
    Ok(IncrementalWallReceiptV1 {
        contract: "codestory.incremental-wall-receipt/v1",
        total_ms,
        core_refresh_ms: u64::from(core.core_refresh_ms),
        retrieval_finalize_ms,
        accounted_ms,
        reconciliation_basis_points: basis_points(accounted_ms, total_ms),
        attributed_ms,
        attributed_basis_points: basis_points(attributed_ms, total_ms),
        phases,
        scheduled_paths: core.scheduled_paths.clone(),
    })
}

#[derive(serde::Serialize)]
struct RetrievalPhaseTimingOutput<'a> {
    phase: &'a str,
    elapsed_ms: u64,
}

#[derive(serde::Serialize)]
struct RetrievalComponentWorkOutput<'a> {
    component: &'a str,
    mode: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retained: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inserted: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reordered: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    predecessor_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attested_bytes: Option<u64>,
}

const RETRIEVAL_PHASE_ALLOWLIST: &[&str] = &[
    "input fingerprint",
    "lexical sidecar",
    "incremental embedded vectors",
    "embedded vectors",
    "vector tokenization",
    "vector native encode",
    "vector normalization",
    "vector sqlite persistence",
    "vector hashing",
    "vector final validation",
    "vector ipc and orchestration",
    "graph artifact",
    "manifest write",
    "unattributed",
];

const RETRIEVAL_COMPONENT_ALLOWLIST: &[&str] = &["lexical", "vectors", "graph"];
const RETRIEVAL_COMPONENT_MODE_ALLOWLIST: &[&str] = &["reused", "copy_on_write", "complete"];

fn safe_retrieval_phase_timings(
    timings: &[FinalizePhaseTiming],
) -> Vec<RetrievalPhaseTimingOutput<'_>> {
    timings
        .iter()
        .filter(|timing| RETRIEVAL_PHASE_ALLOWLIST.contains(&timing.phase.as_str()))
        .map(|timing| RetrievalPhaseTimingOutput {
            phase: &timing.phase,
            elapsed_ms: timing.elapsed_ms,
        })
        .collect()
}

fn safe_retrieval_component_work(
    work: &[FinalizeComponentWork],
) -> Vec<RetrievalComponentWorkOutput<'_>> {
    work.iter()
        .filter(|work| {
            RETRIEVAL_COMPONENT_ALLOWLIST.contains(&work.component.as_str())
                && RETRIEVAL_COMPONENT_MODE_ALLOWLIST.contains(&work.mode.as_str())
        })
        .map(|work| RetrievalComponentWorkOutput {
            component: &work.component,
            mode: &work.mode,
            retained: work.retained,
            inserted: work.inserted,
            removed: work.removed,
            reordered: work.reordered,
            predecessor_bytes: work.predecessor_bytes,
            output_bytes: work.output_bytes,
            attested_bytes: work.attested_bytes,
        })
        .collect()
}

fn emit_retrieval_index(
    format: OutputFormat,
    outcome: &FinalizeIndexOutcome,
    core_phase_timings: Option<&IndexingPhaseTimings>,
    incremental_wall_receipt: Option<&IncrementalWallReceiptV1>,
    output_file: Option<&std::path::Path>,
) -> Result<()> {
    let retrieval_phase_timings = safe_retrieval_phase_timings(&outcome.phase_timings);
    let retrieval_component_work = safe_retrieval_component_work(&outcome.component_work);
    let payload = RetrievalIndexOutput {
        manifest: &outcome.manifest,
        degraded_modes: &outcome.degraded_modes,
        scip_stubbed: outcome.scip_stubbed,
        generation_retention_plan: &outcome.generation_retention_plan,
        generation_retention: &outcome.generation_retention,
        core_phase_timings,
        incremental_wall_receipt,
        retrieval_phase_timings,
        retrieval_component_work,
    };
    let mut markdown = format!(
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
    if let Some(timings) = core_phase_timings {
        markdown.push_str("\n## Core indexing\n\n");
        crate::output::append_index_phase_timings(&mut markdown, timings);
    }
    if let Some(receipt) = incremental_wall_receipt {
        markdown.push_str("\n## Incremental wall receipt\n\n");
        markdown.push_str(&format!(
            "- total: {} ms\n- accounted: {} ms\n- reconciliation: {} bp\n- attributed: {} ms\n- attributed: {} bp\n",
            receipt.total_ms,
            receipt.accounted_ms,
            receipt.reconciliation_basis_points,
            receipt.attributed_ms,
            receipt.attributed_basis_points,
        ));
    }
    markdown.push_str("\n## Retrieval phases\n\n");
    for timing in &payload.retrieval_phase_timings {
        markdown.push_str(&format!("- {}: {} ms\n", timing.phase, timing.elapsed_ms));
    }
    markdown.push_str("\n## Retrieval component work\n\n");
    for work in &payload.retrieval_component_work {
        markdown.push_str(&format!(
            "- {}: mode={}, retained={}, inserted={}, removed={}, reordered={}, predecessor_bytes={}, output_bytes={}, attested_bytes={}\n",
            work.component,
            work.mode,
            work.retained
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.inserted
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.removed
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.reordered
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.predecessor_bytes
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.output_bytes
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            work.attested_bytes
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        ));
    }
    emit(format, &payload, markdown, output_file)
}

fn emit_retrieval_query(
    format: OutputFormat,
    result: &codestory_runtime::QueryResult,
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
    ready_lease: &codestory_runtime::ReadyLeaseEvidence,
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
        "# Retrieval status\n\n- retrieval_mode: `{}`\n- degraded_reason: {:?}\n- query_embedding_backend: `{}`\n- embedding_device_policy: `{}` observed_device=`{}` observation_source=`{}` detected_provider={:?} detected_gpu={:?} accelerator_requested={} accelerator_request_provider={:?} accelerator_request_device={:?} cpu_allowed={}\n- manifest_vector_embedding_backend: `{}` dim={:?}\n- stored_doc_vector_producer: `{}` dim={:?} mixed_backends={:?}\n{}- lexical: {:?} ({:?}) capabilities: lexical={}\n- semantic: {:?} ({:?}) capabilities: semantic={}\n- scip: {:?} ({:?}) capabilities: graph={}\n- ready_lease: present={} admission_basis=`{}` observer_epoch_coherence=`{}` memo_holds_observations={}\n",
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
        ready_lease.ready_lease_present,
        ready_lease.ready_lease_admission_basis,
        ready_lease.ready_lease_observer_epoch_coherence,
        ready_lease.ready_lease_memo_holds_observations,
    );
    emit(
        format,
        &ObservedRetrievalStatus {
            report,
            ready_lease,
        },
        markdown,
        output_file,
    )
}

fn emit_retrieval_inventory(
    format: OutputFormat,
    report: &codestory_runtime::SidecarInventoryReport,
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
    report: &codestory_runtime::SidecarGcReport,
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
    use crate::args::ProjectArgs;
    use crate::status_wire_test_support as wire;
    use anyhow::anyhow;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn retrieval_observability_exposes_only_aggregate_allowlisted_values() {
        let phases = vec![
            FinalizePhaseTiming {
                phase: "lexical sidecar".into(),
                elapsed_ms: 17,
            },
            FinalizePhaseTiming {
                phase: "vector tokenization".into(),
                elapsed_ms: 3,
            },
            FinalizePhaseTiming {
                phase: "vector native encode".into(),
                elapsed_ms: 23,
            },
            FinalizePhaseTiming {
                phase: "vector normalization".into(),
                elapsed_ms: 2,
            },
            FinalizePhaseTiming {
                phase: "vector sqlite persistence".into(),
                elapsed_ms: 7,
            },
            FinalizePhaseTiming {
                phase: "vector hashing".into(),
                elapsed_ms: 5,
            },
            FinalizePhaseTiming {
                phase: "vector final validation".into(),
                elapsed_ms: 11,
            },
            FinalizePhaseTiming {
                phase: "vector ipc and orchestration".into(),
                elapsed_ms: 13,
            },
            FinalizePhaseTiming {
                phase: "/private/source/path".into(),
                elapsed_ms: 99,
            },
        ];
        let work = vec![
            FinalizeComponentWork {
                component: "vectors".into(),
                mode: "copy_on_write".into(),
                retained: Some(41),
                inserted: Some(2),
                removed: Some(1),
                reordered: Some(3),
                predecessor_bytes: Some(1_024),
                output_bytes: Some(1_100),
                attested_bytes: Some(1_100),
            },
            FinalizeComponentWork {
                component: "/private/component".into(),
                mode: "complete".into(),
                retained: None,
                inserted: None,
                removed: None,
                reordered: None,
                predecessor_bytes: None,
                output_bytes: None,
                attested_bytes: None,
            },
        ];

        let safe_phases = safe_retrieval_phase_timings(&phases);
        assert_eq!(safe_phases.len(), 8);
        assert_eq!(safe_phases[0].phase, "lexical sidecar");
        assert_eq!(safe_phases[0].elapsed_ms, 17);
        assert_eq!(safe_phases[1].phase, "vector tokenization");
        assert_eq!(safe_phases[2].phase, "vector native encode");
        assert_eq!(safe_phases[3].phase, "vector normalization");
        assert_eq!(safe_phases[4].phase, "vector sqlite persistence");
        assert_eq!(safe_phases[5].phase, "vector hashing");
        assert_eq!(safe_phases[6].phase, "vector final validation");
        assert_eq!(safe_phases[7].phase, "vector ipc and orchestration");

        let safe_work = safe_retrieval_component_work(&work);
        assert_eq!(safe_work.len(), 1);
        assert_eq!(safe_work[0].component, "vectors");
        assert_eq!(safe_work[0].mode, "copy_on_write");
        assert_eq!(safe_work[0].retained, Some(41));
        assert_eq!(safe_work[0].inserted, Some(2));
        assert_eq!(safe_work[0].removed, Some(1));
        assert_eq!(safe_work[0].reordered, Some(3));
        assert_eq!(safe_work[0].predecessor_bytes, Some(1_024));
        assert_eq!(safe_work[0].output_bytes, Some(1_100));
        assert_eq!(safe_work[0].attested_bytes, Some(1_100));
    }

    #[test]
    fn incremental_wall_receipt_uses_exclusive_phases_and_reconciles_outer_wall() {
        let core = codestory_contracts::api::IncrementalCoreWallTimings {
            core_refresh_ms: 10_000,
            discovery_and_scheduling_ms: 120,
            stage_open_ms: 400,
            parse_and_extraction_ms: 100,
            core_staging_and_mutation_ms: 5_080,
            candidate_sealing_ms: 1_700,
            pointer_publication_ms: 2_400,
            lock_wait_ms: 200,
            unattributed_ms: 0,
            scheduled_paths: vec![codestory_contracts::api::IncrementalScheduledPathDto {
                path: "src/server.c".into(),
                action: codestory_contracts::api::IncrementalScheduledPathActionDto::Index,
                reason: codestory_contracts::api::IncrementalScheduledPathReasonDto::SourceIdentityChanged,
            }],
        };
        let retrieval = vec![
            FinalizePhaseTiming {
                phase: "input fingerprint".into(),
                elapsed_ms: 350,
            },
            FinalizePhaseTiming {
                phase: "lexical sidecar".into(),
                elapsed_ms: 1_902,
            },
            FinalizePhaseTiming {
                phase: "incremental embedded vectors".into(),
                elapsed_ms: 741,
            },
            FinalizePhaseTiming {
                phase: "vector native encode".into(),
                elapsed_ms: 9_999,
            },
            FinalizePhaseTiming {
                phase: "vector ipc and orchestration".into(),
                elapsed_ms: 343,
            },
            FinalizePhaseTiming {
                phase: "graph artifact".into(),
                elapsed_ms: 1_388,
            },
            FinalizePhaseTiming {
                phase: "manifest write".into(),
                elapsed_ms: 1_181,
            },
            FinalizePhaseTiming {
                phase: "unattributed".into(),
                elapsed_ms: 289,
            },
        ];

        let receipt = build_incremental_wall_receipt(16_000, 5_851, &core, &retrieval)
            .expect("build reconciled receipt");
        assert_eq!(receipt.contract, "codestory.incremental-wall-receipt/v1");
        assert_eq!(receipt.total_ms, 16_000);
        assert_eq!(receipt.accounted_ms, 16_000);
        assert_eq!(receipt.reconciliation_basis_points, 10_000);
        assert_eq!(receipt.phases.vector_ms, 398);
        assert_eq!(receipt.phases.process_ipc_ms, 343);
        assert_eq!(receipt.phases.unattributed_ms, 438);
        assert_eq!(receipt.phases.sum(), receipt.total_ms);
        assert_eq!(receipt.scheduled_paths, core.scheduled_paths);
    }

    struct LiveOperationFixture {
        _project: tempfile::TempDir,
        _core_cache: tempfile::TempDir,
        args: ProjectArgs,
        runtime: RuntimeContext,
        project_id: String,
        current_generation: String,
        rollback_generation: String,
    }

    fn live_operation_fixture() -> LiveOperationFixture {
        let project = tempdir().expect("operation project");
        let core_cache = tempdir().expect("operation core cache");
        fs::write(
            project.path().join("lib.rs"),
            "pub fn oracle_anchor_symbol() {}\n",
        )
        .expect("write operation source");
        let args = ProjectArgs {
            project: project.path().to_path_buf(),
            cache_dir: Some(core_cache.path().to_path_buf()),
        };
        let runtime = RuntimeContext::new_inspect_only(&args).expect("operation runtime");
        codestory_retrieval::test_support::publish_empty_complete_core_fixture(
            &runtime.project_root,
            &runtime.storage_path,
        )
        .expect("publish operation core");
        let (current, rollback) =
            codestory_retrieval::test_support::publish_retained_rollback_fixture(
                &runtime.project_root,
                &runtime.storage_path,
                runtime.sidecar.as_raw_config_for_test(),
            )
            .expect("publish retained rollback fixture");
        let project_id = current.project_id.clone();
        let current_generation = current
            .sidecar_generation
            .expect("current fixture generation");
        let rollback_generation = rollback
            .manifest
            .sidecar_generation
            .expect("rollback fixture generation");
        LiveOperationFixture {
            _project: project,
            _core_cache: core_cache,
            args,
            runtime,
            project_id,
            current_generation,
            rollback_generation,
        }
    }

    fn read_rendered_operation(
        emit_json: impl FnOnce(&std::path::Path),
        emit_markdown: impl FnOnce(&std::path::Path),
    ) -> (serde_json::Value, String) {
        let output = tempfile::tempdir().expect("operation output");
        let json_path = output.path().join("operation.json");
        let markdown_path = output.path().join("operation.md");
        emit_json(&json_path);
        emit_markdown(&markdown_path);
        let json =
            serde_json::from_str(&std::fs::read_to_string(json_path).expect("read operation json"))
                .expect("parse operation json");
        let markdown = std::fs::read_to_string(markdown_path).expect("read operation markdown");
        (json, markdown)
    }

    #[test]
    fn pre_change_retrieval_inventory_and_apply_execute_real_cleanup() {
        let retrieval_cache = tempdir().expect("operation retrieval cache");
        codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            let fixture = live_operation_fixture();
            let stale_paths =
                codestory_retrieval::test_support::write_reclaimable_generation_fixture(
                    fixture.runtime.sidecar.as_raw_config_for_test(),
                    &fixture.project_id,
                    "cccccccccccccccc",
                )
                .expect("write reclaimable generation");

            let inventory_path = retrieval_cache.path().join("inventory.json");
            run_retrieval_inventory(RetrievalInventoryCommand {
                project: fixture.args.clone(),
                apply: false,
                format: OutputFormat::Json,
                output_file: Some(inventory_path.clone()),
            })
            .expect("run retrieval inventory");
            let inventory: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(inventory_path).expect("read inventory output"),
            )
            .expect("parse inventory output");
            assert_eq!(inventory["dry_run"], true);
            assert_eq!(inventory["generation_retention"]["reclaimable_bytes"], 24);
            assert!(
                stale_paths.iter().all(|path| path.is_dir()),
                "dry-run inventory mutated the reclaimable generation"
            );

            let apply_path = retrieval_cache.path().join("inventory-apply.json");
            run_retrieval_inventory(RetrievalInventoryCommand {
                project: fixture.args,
                apply: true,
                format: OutputFormat::Json,
                output_file: Some(apply_path.clone()),
            })
            .expect("apply retrieval inventory");
            let applied: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(apply_path).expect("read inventory apply output"),
            )
            .expect("parse inventory apply output");
            assert_eq!(applied["dry_run"], false);
            assert_eq!(applied["generation_retention"]["removed_bytes"], 24);
            assert_eq!(
                applied["generation_retention"]["remaining_reclaimable_bytes"],
                0
            );
            assert!(
                stale_paths.iter().all(|path| !path.exists()),
                "applied inventory left reclaimable generation bytes behind"
            );
        });
    }

    #[test]
    fn pre_change_retrieval_query_executes_zero_dense_fixture_with_stable_hits() {
        let retrieval_cache = tempdir().expect("operation retrieval cache");
        codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            let fixture = live_operation_fixture();
            let query = "oracle_anchor_symbol";
            let direct = codestory_retrieval::execute_retrieval_query_with_cache_for_runtime(
                codestory_retrieval::QueryRequest {
                    project_root: &fixture.runtime.project_root,
                    storage_path: &fixture.runtime.storage_path,
                    query,
                    budget_ms: Some(500),
                    cancelled: None,
                },
                &mut codestory_retrieval::RetrievalCache::new(),
                fixture.runtime.sidecar.as_raw_config_for_test(),
            )
            .expect("execute direct zero-dense query");
            assert!(
                direct.hits.iter().any(|hit| {
                    hit.file_path == "lib.rs"
                        && hit
                            .source_excerpt
                            .as_deref()
                            .is_some_and(|excerpt| excerpt.contains("oracle_anchor_symbol"))
                }),
                "zero-dense fixture did not return the indexed source symbol: {:#?}",
                direct.hits
            );

            let output_path = retrieval_cache.path().join("query.json");
            run_retrieval_query(RetrievalQueryCommand {
                query: query.into(),
                project: fixture.args,
                budget_ms: Some(500),
                format: OutputFormat::Json,
                output_file: Some(output_path.clone()),
            })
            .expect("run retrieval query");
            let output: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(output_path).expect("read query output"))
                    .expect("parse query output");
            assert!(output["trace"]["elapsed_ms"].as_u64().is_some());
            assert_eq!(output["trace"]["retrieval_mode"], "full");
            let direct_hits: serde_json::Value = serde_json::from_str(
                &serde_json::to_string(&direct.hits).expect("serialize direct hits"),
            )
            .expect("parse direct hits");
            assert_eq!(
                output["hits"], direct_hits,
                "CLI query changed zero-dense hits or their order"
            );
        });
    }

    #[test]
    fn pre_change_rollback_dry_run_and_apply_preserve_publication_semantics() {
        let retrieval_cache = tempdir().expect("operation retrieval cache");
        codestory_retrieval::with_test_cache_root(retrieval_cache.path(), || {
            let fixture = live_operation_fixture();
            let before = codestory_retrieval::observe_retained_rollback_generation(
                &fixture.runtime.project_root,
                &fixture.runtime.storage_path,
                fixture.runtime.sidecar.as_raw_config_for_test(),
            )
            .expect("observe retained rollback")
            .expect("retained rollback exists");
            assert_eq!(
                before.current_generation.as_deref(),
                Some(fixture.current_generation.as_str())
            );
            assert_eq!(
                before.rollback_generation.as_deref(),
                Some(fixture.rollback_generation.as_str())
            );

            let dry_run_path = retrieval_cache.path().join("rollback-dry-run.json");
            run_retrieval_activate_rollback(RetrievalActivateRollbackCommand {
                project: fixture.args.clone(),
                dry_run: true,
                format: OutputFormat::Json,
                output_file: Some(dry_run_path.clone()),
            })
            .expect("validate retained rollback");
            let dry_run: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(dry_run_path).expect("read rollback dry-run output"),
            )
            .expect("parse rollback dry-run output");
            assert_eq!(dry_run["outcome"]["applied"], false);
            assert_eq!(
                dry_run["outcome"]["activated_generation"],
                fixture.rollback_generation
            );
            assert_eq!(
                codestory_retrieval::observe_retained_rollback_generation(
                    &fixture.runtime.project_root,
                    &fixture.runtime.storage_path,
                    fixture.runtime.sidecar.as_raw_config_for_test(),
                )
                .expect("re-observe retained rollback after dry-run"),
                Some(before)
            );

            let apply_path = retrieval_cache.path().join("rollback-apply.json");
            run_retrieval_activate_rollback(RetrievalActivateRollbackCommand {
                project: fixture.args,
                dry_run: false,
                format: OutputFormat::Json,
                output_file: Some(apply_path.clone()),
            })
            .expect("activate retained rollback");
            let applied: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(apply_path).expect("read rollback apply output"),
            )
            .expect("parse rollback apply output");
            assert_eq!(applied["outcome"]["applied"], true);
            assert_eq!(
                applied["outcome"]["activated_generation"],
                fixture.rollback_generation
            );
            assert!(
                codestory_retrieval::observe_retained_rollback_generation(
                    &fixture.runtime.project_root,
                    &fixture.runtime.storage_path,
                    fixture.runtime.sidecar.as_raw_config_for_test(),
                )
                .expect("observe after rollback activation")
                .is_none(),
                "applied rollback must consume the retained pointer"
            );
        });
    }

    #[test]
    fn pre_change_retrieval_inventory_and_gc_wires_are_frozen() {
        let inventory = codestory_retrieval::SidecarInventoryReport {
            dry_run: true,
            cache_root: "/cache-root".into(),
            generation_retention: Some(
                serde_json::from_value(serde_json::json!({
                    "dry_run": true,
                    "project_id": "project-fixed",
                    "pruning_suppressed": false,
                    "active_bytes": 11,
                    "rollback_bytes": 13,
                    "building_bytes": 17,
                    "retained_bytes": 41,
                    "reclaimable_bytes": 19,
                    "bundles": [],
                    "blocked": [],
                    "errors": ["fixed inventory warning"]
                }))
                .expect("inventory retention plan"),
            ),
        };
        let (inventory_json, inventory_markdown) = read_rendered_operation(
            |path| {
                emit_retrieval_inventory(OutputFormat::Json, &inventory, Some(path))
                    .expect("emit inventory json")
            },
            |path| {
                emit_retrieval_inventory(OutputFormat::Markdown, &inventory, Some(path))
                    .expect("emit inventory markdown")
            },
        );
        assert_eq!(
            inventory_json,
            serde_json::json!({
                "dry_run": true,
                "cache_root": "/cache-root",
                "generation_retention": {
                    "dry_run": true,
                    "project_id": "project-fixed",
                    "pruning_suppressed": false,
                    "active_bytes": 11,
                    "rollback_bytes": 13,
                    "building_bytes": 17,
                    "retained_bytes": 41,
                    "reclaimable_bytes": 19,
                    "bundles": [],
                    "blocked": [],
                    "errors": ["fixed inventory warning"]
                }
            })
        );
        assert_eq!(
            inventory_markdown,
            "# Retrieval runtime inventory\n\n- dry_run: true\n- cache_root: `/cache-root`\n- generation_retention_active_bytes: 11\n- generation_retention_rollback_bytes: 13\n- generation_retention_building_bytes: 17\n- generation_retention_retained_bytes: 41\n- generation_retention_reclaimable_bytes: 19\n- generation_retention_pruning_suppressed: false\n- generation_retention_errors: `fixed inventory warning`\n"
        );

        let gc = codestory_retrieval::SidecarGcReport {
            dry_run: false,
            cache_root: "/cache-root".into(),
            generation_retention: Some(
                serde_json::from_value(serde_json::json!({
                    "dry_run": false,
                    "project_id": "project-fixed",
                    "pruning_suppressed": false,
                    "active_bytes": 11,
                    "rollback_bytes": 13,
                    "building_bytes": 17,
                    "retained_bytes": 41,
                    "reclaimable_bytes": 19,
                    "removed_bytes": 7,
                    "remaining_reclaimable_bytes": 12,
                    "removals": [{
                        "generation": "old-generation",
                        "semantic_generation": "old-semantic",
                        "removed_paths": ["/cache-root/old-generation"],
                        "semantic_generation_removed": true,
                        "removed_bytes": 7,
                        "remaining_reclaimable_bytes": 12,
                        "errors": []
                    }],
                    "errors": ["fixed gc warning"]
                }))
                .expect("gc retention report"),
            ),
        };
        let (gc_json, gc_markdown) = read_rendered_operation(
            |path| emit_retrieval_gc(OutputFormat::Json, &gc, Some(path)).expect("emit gc json"),
            |path| {
                emit_retrieval_gc(OutputFormat::Markdown, &gc, Some(path))
                    .expect("emit gc markdown")
            },
        );
        assert_eq!(
            gc_json,
            serde_json::json!({
                "dry_run": false,
                "cache_root": "/cache-root",
                "generation_retention": {
                    "dry_run": false,
                    "project_id": "project-fixed",
                    "pruning_suppressed": false,
                    "active_bytes": 11,
                    "rollback_bytes": 13,
                    "building_bytes": 17,
                    "retained_bytes": 41,
                    "reclaimable_bytes": 19,
                    "removed_bytes": 7,
                    "remaining_reclaimable_bytes": 12,
                    "removals": [{
                        "generation": "old-generation",
                        "semantic_generation": "old-semantic",
                        "removed_paths": ["/cache-root/old-generation"],
                        "semantic_generation_removed": true,
                        "removed_bytes": 7,
                        "remaining_reclaimable_bytes": 12,
                        "errors": []
                    }],
                    "errors": ["fixed gc warning"]
                }
            })
        );
        assert_eq!(
            gc_markdown,
            "# Retrieval runtime GC\n\n- dry_run: false\n- cache_root: `/cache-root`\n- generation_retention_active_bytes: 11\n- generation_retention_rollback_bytes: 13\n- generation_retention_building_bytes: 17\n- generation_retention_retained_bytes: 41\n- generation_retention_reclaimable_bytes: 19\n- generation_retention_removed_bytes: 7\n- generation_retention_remaining_reclaimable_bytes: 12\n- generation_retention_pruning_suppressed: false\n- generation_retention_errors: `fixed gc warning`\n"
        );
    }

    #[test]
    fn pre_change_retrieval_query_wire_preserves_hit_order_and_elapsed_type() {
        let result = codestory_retrieval::QueryResult {
            publication_identity: None,
            query: "find fixed symbol".into(),
            features: codestory_retrieval::classify_query("find fixed symbol"),
            hits: vec![
                codestory_retrieval::CandidateHit::lexical_stub("src/first.rs", 0.875),
                codestory_retrieval::CandidateHit::with_source(
                    "src/second.rs",
                    Some("second_symbol".into()),
                    0.625,
                    codestory_retrieval::CandidateSource::Semantic,
                ),
            ],
            trace: codestory_retrieval::QueryTrace {
                retrieval_mode: "full".into(),
                degraded_reason: None,
                total_budget_ms: 500,
                elapsed_ms: 37,
                cancel_reason: None,
                cache_hit: false,
                stages: Vec::new(),
            },
        };
        let (json, markdown) = read_rendered_operation(
            |path| {
                emit_retrieval_query(OutputFormat::Json, &result, Some(path))
                    .expect("emit query json")
            },
            |path| {
                emit_retrieval_query(OutputFormat::Markdown, &result, Some(path))
                    .expect("emit query markdown")
            },
        );
        assert_eq!(json["trace"]["elapsed_ms"].as_u64(), Some(37));
        assert_eq!(json["hits"][0]["file_path"], "src/first.rs");
        assert_eq!(json["hits"][1]["file_path"], "src/second.rs");
        assert_eq!(
            json,
            serde_json::json!({
                "query": "find fixed symbol",
                "features": {
                    "raw_query": "find fixed symbol",
                    "shape": "natural_language",
                    "token_count": 3,
                    "has_path_separators": false,
                    "has_camel_case_token": false,
                    "has_snake_case_token": false,
                    "looks_like_qualified_symbol": false
                },
                "hits": [{
                    "file_path": "src/first.rs",
                    "symbol_name": null,
                    "start_line": null,
                    "score": 0.875,
                    "source": "lexical",
                    "provenance": ["lexical_source"]
                }, {
                    "file_path": "src/second.rs",
                    "symbol_name": "second_symbol",
                    "start_line": null,
                    "score": 0.625,
                    "source": "semantic"
                }],
                "trace": {
                    "retrieval_mode": "full",
                    "total_budget_ms": 500,
                    "elapsed_ms": 37,
                    "cache_hit": false,
                    "stages": []
                }
            })
        );
        assert_eq!(
            markdown,
            "# Retrieval query\n\n- query: `find fixed symbol`\n- shape: `NaturalLanguage`\n- retrieval_mode: `full`\n- hits: 2\n- top: src/first.rs (0.875)\n- elapsed_ms: 37\n"
        );
    }

    #[test]
    fn pre_change_rollback_wires_and_typed_refusal_chain_are_frozen() {
        fn outcome(applied: bool) -> codestory_retrieval::RollbackActivationOutcome {
            codestory_retrieval::RollbackActivationOutcome {
                project_id: "project-fixed".into(),
                applied,
                previous_generation: Some("current-generation".into()),
                previous_semantic_generation: "current-semantic".into(),
                activated_generation: "rollback-generation".into(),
                activated_semantic_generation: "rollback-semantic".into(),
                activated_built_at_epoch_ms: 101,
                rollback_verified_at_epoch_ms: 202,
                rollback_pointer_retained: false,
                activated_retrieval_mode: "full".into(),
            }
        }

        for applied in [false, true] {
            let outcome = outcome(applied);
            let project = "/repo";
            let next_commands = rollback_activation_next_commands(project, &outcome);
            let markdown = render_rollback_activation_markdown(project, &outcome, &next_commands);
            let payload = RetrievalActivateRollbackOutput {
                project: project.into(),
                outcome,
                next_commands,
            };
            let (json, emitted_markdown) = read_rendered_operation(
                |path| {
                    emit(OutputFormat::Json, &payload, markdown.clone(), Some(path))
                        .expect("emit rollback json")
                },
                |path| {
                    emit(
                        OutputFormat::Markdown,
                        &payload,
                        markdown.clone(),
                        Some(path),
                    )
                    .expect("emit rollback markdown")
                },
            );
            let expected_commands = if applied {
                serde_json::json!([
                    "codestory-cli doctor --project \"/repo\" --format markdown",
                    "codestory-cli retrieval inventory --project \"/repo\" --apply"
                ])
            } else {
                serde_json::json!(["codestory-cli retrieval activate-rollback --project \"/repo\""])
            };
            assert_eq!(
                json,
                serde_json::json!({
                    "project": "/repo",
                    "outcome": {
                        "project_id": "project-fixed",
                        "applied": applied,
                        "previous_generation": "current-generation",
                        "previous_semantic_generation": "current-semantic",
                        "activated_generation": "rollback-generation",
                        "activated_semantic_generation": "rollback-semantic",
                        "activated_built_at_epoch_ms": 101,
                        "rollback_verified_at_epoch_ms": 202,
                        "rollback_pointer_retained": false,
                        "activated_retrieval_mode": "full"
                    },
                    "next_commands": expected_commands
                })
            );
            let expected_markdown = if applied {
                "# Retrieval rollback activation\n\n- project: `/repo`\n- project_id: `project-fixed`\n- applied: true\n- previous_generation: `current-generation`\n- activated_generation: `rollback-generation`\n- activated_semantic_generation: `rollback-semantic`\n- activated_retrieval_mode: `full`\n- rollback_pointer_retained: false\n\n## Next\n\n- `codestory-cli doctor --project \"/repo\" --format markdown`\n- `codestory-cli retrieval inventory --project \"/repo\" --apply`\n"
            } else {
                "# Retrieval rollback activation\n\n- project: `/repo`\n- project_id: `project-fixed`\n- applied: false\n- previous_generation: `current-generation`\n- activated_generation: `rollback-generation`\n- activated_semantic_generation: `rollback-semantic`\n- activated_retrieval_mode: `full`\n- rollback_pointer_retained: false\n\nValidation only: the current retrieval generation was not changed. Rerun without `--dry-run` to activate.\n\n## Next\n\n- `codestory-cli retrieval activate-rollback --project \"/repo\"`\n"
            };
            assert_eq!(emitted_markdown, expected_markdown);
        }

        let error = annotate_rollback_activation_error(
            codestory_retrieval::RollbackActivationError::Refused(
                codestory_retrieval::RollbackActivationRefusal::RollbackEvidenceInvalid {
                    reason: "fixed evidence mismatch".into(),
                },
            ),
        );
        assert_eq!(
            error.chain().map(ToString::to_string).collect::<Vec<_>>(),
            vec![
                "retrieval activate-rollback refused: rollback_evidence_invalid",
                "rollback_evidence_invalid: fixed evidence mismatch",
            ]
        );
    }

    fn rendered_status_case(
        report: &RetrievalStatusReport,
        ready_lease: &codestory_runtime::ReadyLeaseEvidence,
    ) -> serde_json::Value {
        let output = tempfile::tempdir().expect("status output");
        let json_path = output.path().join("status.json");
        let markdown_path = output.path().join("status.md");
        emit_retrieval_status(OutputFormat::Json, report, ready_lease, Some(&json_path))
            .expect("emit status json");
        emit_retrieval_status(
            OutputFormat::Markdown,
            report,
            ready_lease,
            Some(&markdown_path),
        )
        .expect("emit status markdown");
        let json_text = std::fs::read_to_string(json_path).expect("read status json");
        let markdown = std::fs::read_to_string(markdown_path).expect("read status markdown");
        serde_json::json!({
            "json": serde_json::from_str::<serde_json::Value>(&json_text)
                .expect("parse status json"),
            "markdown": markdown,
        })
    }

    #[test]
    fn pre_change_status_wire_retrieval_json_and_markdown_are_non_vacuous() {
        let healthy = wire::healthy_status_report();
        let degraded = wire::degraded_status_report();
        let unavailable = wire::unavailable_status_report();
        assert!(healthy.is_live_ready());
        assert!(
            !degraded.is_live_ready(),
            "full plus a degraded reason must not be live-ready"
        );
        let cases = serde_json::json!({
            "healthy": rendered_status_case(&healthy, &wire::ready_lease_evidence()),
            "degraded": rendered_status_case(&degraded, &wire::stale_ready_lease_evidence()),
            "unavailable": rendered_status_case(&unavailable, &codestory_runtime::ReadyLeaseEvidence::default()),
            "probe_error": {
                "error": retrieval_status_error::<RetrievalStatusReport>(wire::probe_error())
                    .expect_err("probe error must remain an error")
                    .chain()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            },
        });
        let raw_cases = cases
            .as_object()
            .expect("retrieval cases")
            .values()
            .filter_map(|case| case.get("json").cloned())
            .collect::<Vec<_>>();
        wire::assert_non_null_coverage(
            &raw_cases,
            &wire::RAW_STATUS_FIELDS,
            "raw retrieval status",
        );
        let union = raw_cases
            .iter()
            .flat_map(|case| case.as_object().expect("raw status object").keys())
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            union,
            wire::RAW_STATUS_FIELDS.into_iter().collect(),
            "raw retrieval status field set drifted"
        );
        wire::assert_json_golden(&cases, wire::RETRIEVAL_GOLDEN, "retrieval status");
    }

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
        let timings =
            run_retrieval_index_refresh(&runtime, RefreshMode::Auto, decision.effective_mode)
                .expect("run compatible incremental refresh")
                .expect("executed core refresh must retain its phase timings");
        assert!(timings.incremental_plan_probe.is_some());
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
