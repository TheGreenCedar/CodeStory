use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, ReadinessGoalDto, ReadinessStatusDto,
    RetrievalFallbackReasonDto,
};
use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;

use crate::{
    args::{DoctorCheckOutput, DoctorOutput, ReadinessLaneOutput, RetrievalStatusOutput},
    display, embedding_config, readiness,
    runtime::RuntimeContext,
};

use super::readiness_commands::doctor_sidecar_status_is_live_ready;
#[cfg(test)]
use super::resolution::quote_command_path;

pub(super) fn build_doctor_output(
    runtime: &RuntimeContext,
    summary: &codestory_contracts::api::ProjectSummary,
) -> DoctorOutput {
    let indexed = summary.stats.node_count > 0;
    let mut retrieval = summary.retrieval.clone();
    if let Some(retrieval) = retrieval.as_mut()
        && let Some(message) = retrieval.fallback_message.as_mut()
    {
        *message = redact_urls_in_text(message);
    }
    let project = display::clean_path_string(&summary.root);
    let storage_path = display::clean_path_string(&runtime.storage_path.to_string_lossy());
    let storage_exists = runtime.storage_path.exists();
    let sidecar_retrieval = doctor_sidecar_status(runtime);
    let readiness_sidecar = agent_readiness_status(runtime, None);
    let readiness = build_summary_readiness(
        &project,
        &summary.stats,
        summary.freshness.as_ref(),
        &readiness_sidecar,
    );
    let readiness_lanes =
        build_readiness_lanes_for_runtime(runtime, &readiness, None, Some(&readiness_sidecar));
    let next_commands = readiness::compatibility_next_commands(&readiness);
    let mut checks = Vec::new();
    checks.push(doctor_check(
        "project",
        "ok",
        format!("Project root resolved to `{project}`."),
    ));
    checks.push(if storage_exists {
        doctor_check(
            "cache",
            "ok",
            format!("Cache database exists at `{storage_path}`."),
        )
    } else {
        doctor_check(
            "cache",
            "warn",
            "Cache database does not exist yet; run `codestory-cli index --refresh full`."
                .to_string(),
        )
    });
    checks.push(if indexed {
        doctor_check(
            "index",
            "ok",
            format!(
                "Indexed {} files, {} nodes, {} edges.",
                summary.stats.file_count, summary.stats.node_count, summary.stats.edge_count
            ),
        )
    } else {
        doctor_check(
            "index",
            "warn",
            "No indexed symbols are available yet.".to_string(),
        )
    });
    checks.push(doctor_sidecar_check(&readiness_sidecar));
    if let Some(retrieval) = retrieval.as_ref()
        && retrieval.stored_embedding.is_some()
    {
        checks.push(semantic_contract_check(retrieval));
    }
    if let Some(freshness) = summary.freshness.as_ref() {
        checks.push(index_freshness_check(freshness));
    }

    let environment = [
        "CODESTORY_EMBED_ALLOW_CPU",
        "CODESTORY_STORED_VECTOR_ENCODING",
        "CODESTORY_HYBRID_RETRIEVAL_ENABLED",
        "CODESTORY_SEMANTIC_DOC_ALIAS_MODE",
    ]
    .into_iter()
    .map(|name| match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            doctor_check(name, "ok", doctor_env_check_message(name, &value))
        }
        _ => doctor_check(name, "info", "not set; using runtime defaults".to_string()),
    })
    .collect::<Vec<_>>();

    DoctorOutput {
        project: project.clone(),
        storage_path,
        indexed,
        stats: summary.stats.clone(),
        retrieval_mode: readiness_sidecar.retrieval_mode.clone(),
        degraded_reason: readiness_sidecar.degraded_reason.clone(),
        sidecar_retrieval,
        retrieval,
        freshness: summary.freshness.clone(),
        readiness,
        readiness_lanes,
        checks,
        next_commands,
        environment,
    }
}

pub(crate) fn build_summary_readiness(
    project: &str,
    stats: &codestory_contracts::api::StorageStatsDto,
    freshness: Option<&IndexFreshnessDto>,
    sidecar: &RetrievalStatusOutput,
) -> Vec<codestory_contracts::api::ReadinessVerdictDto> {
    readiness::build_readiness_verdicts(readiness::ReadinessInputs {
        project,
        stats,
        freshness,
        sidecar: Some(readiness_sidecar_input(sidecar)),
    })
}

pub(super) fn readiness_sidecar_input(
    sidecar: &RetrievalStatusOutput,
) -> readiness::ReadinessSidecarInput<'_> {
    readiness::ReadinessSidecarInput {
        profile: sidecar.profile.as_deref(),
        run_id: sidecar.run_id.as_deref(),
        retrieval_mode: sidecar.retrieval_mode.as_str(),
        degraded_reason: sidecar.degraded_reason.as_deref(),
        embedding_device_policy: Some(sidecar.embedding_device_policy.as_str()),
        embedding_device_state: Some(sidecar.embedding_device_state.as_str()),
        embedding_device_observation_source: Some(
            sidecar.embedding_device_observation_source.as_str(),
        ),
        embedding_detected_provider: sidecar.embedding_detected_provider.as_deref(),
        embedding_detected_gpu: sidecar.embedding_detected_gpu.as_deref(),
        embedding_accelerator_requested: sidecar.embedding_accelerator_requested,
        embedding_accelerator_request_provider: sidecar
            .embedding_accelerator_request_provider
            .as_deref(),
        embedding_accelerator_request_device: sidecar
            .embedding_accelerator_request_device
            .as_deref(),
        embedding_cpu_allowed: sidecar.embedding_cpu_allowed,
        manifest_generation: sidecar.manifest_generation.as_deref(),
        manifest_input_hash: sidecar.manifest_input_hash.as_deref(),
    }
}

pub(crate) fn doctor_sidecar_status(runtime: &RuntimeContext) -> RetrievalStatusOutput {
    let sidecar = runtime.sidecar.clone();
    match codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    ) {
        Ok(report) => doctor_sidecar_status_from_report(report, Some(&sidecar)),
        Err(error) => doctor_sidecar_status_error(error, Some(&sidecar)),
    }
}

pub(super) fn doctor_sidecar_status_for_runtime(
    runtime: &RuntimeContext,
    sidecar: codestory_retrieval::SidecarRuntimeConfig,
) -> RetrievalStatusOutput {
    match codestory_retrieval::strict_sidecar_status_for_runtime(
        &runtime.project_root,
        Some(&runtime.storage_path),
        sidecar.clone(),
    ) {
        Ok(report) => doctor_sidecar_status_from_report(report, Some(&sidecar)),
        Err(error) => doctor_sidecar_status_error(error, Some(&sidecar)),
    }
}

pub(super) fn doctor_sidecar_status_from_report(
    report: codestory_retrieval::RetrievalStatusReport,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> RetrievalStatusOutput {
    let manifest_generation = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_generation.clone());
    let manifest_input_hash = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.sidecar_input_hash.clone());
    let precise_semantic_import_status = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_status.clone());
    let precise_semantic_import_reason = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_reason.clone());
    let precise_semantic_import_revision = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_revision.clone());
    let precise_semantic_import_producer = report
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.precise_semantic_import_producer.clone());
    RetrievalStatusOutput {
        profile: runtime.map(|runtime| runtime.profile.as_str().to_string()),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: report.retrieval_mode,
        degraded_reason: report.degraded_reason,
        embedding_device_policy: report.embedding_device_policy,
        embedding_device_state: report.embedding_device_state,
        embedding_device_observation_source: report.embedding_device_observation_source,
        embedding_detected_provider: report.embedding_detected_provider,
        embedding_detected_gpu: report.embedding_detected_gpu,
        embedding_accelerator_requested: report.embedding_accelerator_requested,
        embedding_accelerator_request_provider: report.embedding_accelerator_request_provider,
        embedding_accelerator_request_device: report.embedding_accelerator_request_device,
        embedding_cpu_allowed: report.embedding_cpu_allowed,
        manifest_generation,
        manifest_input_hash,
        precise_semantic_import_status,
        precise_semantic_import_reason,
        precise_semantic_import_revision,
        precise_semantic_import_producer,
    }
}

pub(super) fn doctor_sidecar_status_error(
    error: anyhow::Error,
    runtime: Option<&codestory_retrieval::SidecarRuntimeConfig>,
) -> RetrievalStatusOutput {
    RetrievalStatusOutput {
        profile: runtime.map(|runtime| runtime.profile.as_str().to_string()),
        run_id: runtime.and_then(|runtime| runtime.run_id.clone()),
        retrieval_mode: "unavailable".to_string(),
        degraded_reason: Some(format!("retrieval_status_error: {error}")),
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
    }
}

pub(super) fn agent_readiness_status(
    runtime: &RuntimeContext,
    run_id: Option<&str>,
) -> RetrievalStatusOutput {
    let agent_runtime = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        codestory_retrieval::SidecarProfile::Agent,
        run_id,
    );
    doctor_sidecar_status_for_runtime(runtime, agent_runtime)
}

pub(crate) fn build_readiness_lanes_for_runtime(
    runtime: &RuntimeContext,
    readiness: &[codestory_contracts::api::ReadinessVerdictDto],
    agent_run_id: Option<&str>,
    selected_agent_status: Option<&RetrievalStatusOutput>,
) -> BTreeMap<String, ReadinessLaneOutput> {
    let project = display::clean_path_string(&runtime.project_root.to_string_lossy());
    let project_arg = display::quote_command_argument_value(&project);
    let local_runtime = runtime.sidecar.with_profile_and_run_id(
        Some(&runtime.project_root),
        codestory_retrieval::SidecarProfile::Local,
        None,
    );
    let local_status = doctor_sidecar_status_for_runtime(runtime, local_runtime);
    let agent_status = selected_agent_status.cloned().unwrap_or_else(|| {
        doctor_sidecar_status_for_runtime(
            runtime,
            runtime.sidecar.with_profile_and_run_id(
                Some(&runtime.project_root),
                codestory_retrieval::SidecarProfile::Agent,
                agent_run_id,
            ),
        )
    });
    let agent_verdict = readiness
        .iter()
        .find(|verdict| verdict.goal == ReadinessGoalDto::AgentPacketSearch);
    let mut lanes = BTreeMap::new();
    lanes.insert(
        "local_default".to_string(),
        readiness_lane_output("local_default", &local_status, None, &project_arg),
    );
    lanes.insert(
        "agent_packet_search".to_string(),
        readiness_lane_output(
            "agent_packet_search",
            &agent_status,
            agent_verdict,
            &project_arg,
        ),
    );
    lanes
}

#[cfg(test)]
pub(super) fn agent_readiness_sidecar_runtime(
    project_root: &Path,
    run_id: Option<&str>,
) -> codestory_retrieval::SidecarRuntimeConfig {
    crate::sidecar_runtime::for_project_with_run_id(
        project_root,
        codestory_retrieval::SidecarProfile::Agent,
        run_id,
    )
}

pub(super) fn readiness_lane_output(
    lane: &str,
    sidecar: &RetrievalStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> ReadinessLaneOutput {
    let status = readiness_lane_status(sidecar, verdict);
    ReadinessLaneOutput {
        status,
        profile: sidecar
            .profile
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        run_id: sidecar.run_id.clone(),
        namespace: None,
        phase: None,
        repair_updated_at_epoch_ms: None,
        retrieval_mode: sidecar.retrieval_mode.clone(),
        degraded_reason: sidecar.degraded_reason.clone(),
        next_command: lane_next_command(lane, sidecar, status, verdict, project_arg),
    }
}

pub(super) fn readiness_lane_status(
    sidecar: &RetrievalStatusOutput,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
) -> ReadinessStatusDto {
    let sidecar_status = if doctor_sidecar_status_is_live_ready(sidecar) {
        ReadinessStatusDto::Ready
    } else {
        ReadinessStatusDto::RepairRetrieval
    };
    if sidecar.profile.as_deref() == Some("agent")
        && sidecar_status == ReadinessStatusDto::RepairRetrieval
        && sidecar
            .degraded_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("embedding_runtime_unavailable:"))
    {
        return ReadinessStatusDto::RepairRetrieval;
    }
    match verdict.map(|verdict| verdict.status) {
        Some(ReadinessStatusDto::Blocked) => ReadinessStatusDto::Blocked,
        Some(status @ (ReadinessStatusDto::RepairSetup | ReadinessStatusDto::RepairIndex)) => {
            status
        }
        Some(ReadinessStatusDto::CheckIndex) if sidecar_status == ReadinessStatusDto::Ready => {
            ReadinessStatusDto::CheckIndex
        }
        _ => sidecar_status,
    }
}

pub(super) fn lane_next_command(
    lane: &str,
    sidecar: &RetrievalStatusOutput,
    status: ReadinessStatusDto,
    verdict: Option<&codestory_contracts::api::ReadinessVerdictDto>,
    project_arg: &str,
) -> Option<String> {
    if status == ReadinessStatusDto::Ready {
        return Some(retrieval_status_command(sidecar, project_arg));
    }
    if let Some(command) = verdict.and_then(|verdict| verdict.minimum_next.first()) {
        return Some(command.clone());
    }
    match lane {
        "agent_packet_search" if !doctor_sidecar_status_is_live_ready(sidecar) => Some(format!(
            "codestory-cli retrieval index --project {project_arg} --profile agent --refresh auto --format json"
        )),
        "local_default" if !doctor_sidecar_status_is_live_ready(sidecar) => Some(format!(
            "codestory-cli retrieval index --project {project_arg} --profile local --refresh full --format json"
        )),
        _ => Some(retrieval_status_command(sidecar, project_arg)),
    }
}

pub(super) fn retrieval_status_command(
    sidecar: &RetrievalStatusOutput,
    project_arg: &str,
) -> String {
    let mut command = format!(
        "codestory-cli retrieval status --project {project_arg} --profile {}",
        sidecar.profile.as_deref().unwrap_or("local")
    );
    if let Some(run_id) = sidecar.run_id.as_deref() {
        command.push_str(" --run-id ");
        command.push_str(&display::quote_command_argument_value(run_id));
    }
    command.push_str(" --format json");
    command
}

pub(super) fn doctor_env_check_message(name: &str, value: &str) -> String {
    let trimmed = value.trim();
    if name.ends_with("_URL") || trimmed.contains("://") {
        return format!(
            "set to `{}`",
            embedding_config::redact_url_for_display(trimmed)
        );
    }
    format!("set to `{trimmed}`")
}

pub(super) fn redact_urls_in_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_url_token)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn redact_url_token(token: &str) -> String {
    let prefix_len = token
        .find("://")
        .and_then(|scheme_end| {
            token[..scheme_end]
                .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')))
                .map(|index| index + 1)
                .or(Some(0))
        })
        .unwrap_or(token.len());
    if prefix_len == token.len() {
        return token.to_string();
    }

    let prefix = &token[..prefix_len];
    let url_and_suffix = &token[prefix_len..];
    let suffix_start = url_and_suffix
        .find([')', ']', '}', ',', ';', '`'])
        .unwrap_or(url_and_suffix.len());
    let (url, suffix) = url_and_suffix.split_at(suffix_start);
    format!(
        "{prefix}{}{suffix}",
        embedding_config::redact_url_for_display(url)
    )
}

pub(super) fn index_freshness_check(freshness: &IndexFreshnessDto) -> DoctorCheckOutput {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => doctor_check(
            "index_freshness",
            "ok",
            format!(
                "Indexed file inventory is fresh (checked={} duration_ms={}).",
                freshness.checked_file_count, freshness.duration_ms
            ),
        ),
        IndexFreshnessStatusDto::Stale => doctor_check(
            "index_freshness",
            "warn",
            format!(
                "Indexed file inventory is stale: changed={} new={} removed={} (checked={} duration_ms={}). Run `codestory-cli index --refresh incremental` to update the cache.",
                freshness.changed_file_count,
                freshness.new_file_count,
                freshness.removed_file_count,
                freshness.checked_file_count,
                freshness.duration_ms
            ),
        ),
        IndexFreshnessStatusDto::NotChecked => doctor_check(
            "index_freshness",
            "info",
            format!(
                "Index freshness was not checked: {}.",
                freshness.reason.as_deref().unwrap_or("no reason reported")
            ),
        ),
    }
}

pub(super) fn semantic_contract_check(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
) -> DoctorCheckOutput {
    let Some(stored) = retrieval.stored_embedding.as_ref() else {
        return doctor_check(
            "semantic_contract",
            "info",
            "Stored semantic doc metadata is unavailable.".to_string(),
        );
    };
    if stored.doc_count == 0 {
        return doctor_check(
            "semantic_contract",
            "info",
            "No stored semantic docs are available to compare with the current embedding config."
                .to_string(),
        );
    }

    let mut gaps = Vec::new();
    if stored.mixed_embedding_profiles {
        gaps.push("stored docs use mixed embedding profiles".to_string());
    }
    if stored.mixed_embedding_models {
        gaps.push("stored docs use mixed cache keys".to_string());
    }
    if stored.mixed_embedding_backends {
        gaps.push("stored docs use mixed embedding backends".to_string());
    }
    if stored.mixed_dimensions {
        gaps.push("stored docs use mixed embedding dimensions".to_string());
    }
    if stored.mixed_doc_versions {
        gaps.push("stored docs use mixed semantic doc versions".to_string());
    }
    if stored.mixed_doc_shapes {
        gaps.push("stored docs use mixed semantic doc shapes".to_string());
    }

    if let Some(current) = retrieval.current_embedding.as_ref() {
        compare_contract_field(
            &mut gaps,
            "embedding profile",
            stored.embedding_profile.as_deref(),
            Some(current.profile.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "embedding backend",
            stored.embedding_backend.as_deref(),
            Some(current.backend.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "cache key",
            stored.cache_key.as_deref(),
            Some(current.cache_key.as_str()),
        );
        compare_contract_field(
            &mut gaps,
            "semantic doc shape",
            stored.doc_shape.as_deref(),
            Some(current.doc_shape.as_str()),
        );
        if let (Some(stored_dim), Some(current_dim)) = (stored.dimension, current.dimension)
            && stored_dim != current_dim
        {
            gaps.push(format!(
                "embedding dimension mismatch: stored={stored_dim} current={current_dim}"
            ));
        }
    } else {
        gaps.push("current embedding config could not be resolved".to_string());
    }

    if gaps.is_empty() {
        doctor_check(
            "semantic_contract",
            "ok",
            format!(
                "semantic ok: stored semantic docs match the current embedding contract (docs={}).",
                stored.doc_count
            ),
        )
    } else if !retrieval.semantic_ready
        && retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    {
        doctor_check(
            "semantic_contract",
            "info",
            format!(
                "semantic stale: {}. Run `codestory-cli retrieval index --refresh full`; the embedded engine initializes automatically.",
                gaps.join("; ")
            ),
        )
    } else {
        doctor_check(
            "semantic_contract",
            "warn",
            format!(
                "semantic stale: {}. Run `codestory-cli retrieval index --refresh full` before trusting packet/search evidence.",
                gaps.join("; ")
            ),
        )
    }
}

pub(super) fn compare_contract_field(
    gaps: &mut Vec<String>,
    label: &str,
    stored: Option<&str>,
    current: Option<&str>,
) {
    match (stored, current) {
        (Some(stored), Some(current)) if stored != current => {
            gaps.push(format!(
                "{label} mismatch: stored={stored} current={current}"
            ));
        }
        (None, Some(current)) => {
            gaps.push(format!(
                "{label} missing from stored docs; current={current}"
            ));
        }
        _ => {}
    }
}

pub(super) fn doctor_check(
    name: impl Into<String>,
    status: impl Into<String>,
    message: impl Into<String>,
) -> DoctorCheckOutput {
    DoctorCheckOutput {
        name: name.into(),
        status: status.into(),
        message: message.into(),
    }
}

pub(super) fn doctor_sidecar_check(sidecar: &RetrievalStatusOutput) -> DoctorCheckOutput {
    if doctor_sidecar_status_is_live_ready(sidecar) {
        let device_note = if sidecar.embedding_cpu_allowed {
            format!(
                " embedding device policy allows CPU-backed mode (observed_device={}).",
                sidecar.embedding_device_state
            )
        } else {
            format!(
                " embedding device policy={} observed_device={}.",
                sidecar.embedding_device_policy, sidecar.embedding_device_state
            )
        };
        return doctor_check(
            "sidecar_retrieval",
            "ok",
            format!("retrieval is ready for packet/search evidence.{device_note}"),
        );
    }

    let reason = sidecar
        .degraded_reason
        .as_deref()
        .unwrap_or("no degraded_reason reported");
    doctor_check(
        "sidecar_retrieval",
        "error",
        format!(
            "retrieval is not ready (mode={} reason={reason}; embedding_device_policy={} observed_device={} cpu_allowed={}); packet/search evidence remains blocked.",
            sidecar.retrieval_mode,
            sidecar.embedding_device_policy,
            sidecar.embedding_device_state,
            sidecar.embedding_cpu_allowed
        ),
    )
}

#[cfg(test)]
pub(super) fn index_next_commands(
    project: &str,
    retrieval: Option<&codestory_contracts::api::RetrievalStateDto>,
    freshness: Option<&IndexFreshnessDto>,
    sidecar_is_full: bool,
) -> Vec<String> {
    let project = quote_command_path(std::path::Path::new(project));
    let mut commands = Vec::new();
    if let Some(freshness) = freshness {
        match freshness.status {
            IndexFreshnessStatusDto::Stale => {
                commands.push(format!(
                    "codestory-cli index --project {project} --refresh incremental"
                ));
                commands.push(format!(
                    "codestory-cli doctor --project {project} --format markdown"
                ));
                return commands;
            }
            IndexFreshnessStatusDto::NotChecked => {
                commands.push(format!(
                    "codestory-cli index --project {project} --refresh full"
                ));
                commands.push(format!(
                    "codestory-cli doctor --project {project} --format markdown"
                ));
                return commands;
            }
            IndexFreshnessStatusDto::Fresh => {}
        }
    }
    if !sidecar_is_full {
        commands.push(format!(
            "codestory-cli retrieval status --project {project}"
        ));
        commands.push(format!(
            "codestory-cli retrieval index --project {project} --refresh full"
        ));
        commands.push(format!(
            "codestory-cli doctor --project {project} --format markdown"
        ));
        return commands;
    }
    if let Some(retrieval) = retrieval.filter(|state| !state.semantic_ready)
        && retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::MissingEmbeddingRuntime)
    {
        commands.push(format!(
            "codestory-cli retrieval index --project {project} --refresh full"
        ));
    }
    commands.push(format!("codestory-cli ground --project {project}"));
    commands.push(format!(
        "codestory-cli search --project {project} --query \"<symbol/file/literal/API path>\" --why"
    ));
    commands.push(format!(
        "codestory-cli context --project {project} --query \"<concrete target>\""
    ));
    commands
}
