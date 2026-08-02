use super::super::readiness_commands::doctor_sidecar_status_is_live_ready;
#[cfg(test)]
use super::super::resolution::quote_command_path;
use super::readiness::{agent_readiness_status, build_readiness_lanes_for_runtime};
use super::sidecar::{build_summary_readiness, doctor_sidecar_status};
use crate::args::{DoctorCheckOutput, DoctorOutput, RetrievalStatusOutput};
use crate::display;
use crate::embedding_config;
use crate::readiness;
use crate::runtime::RuntimeContext;
use codestory_contracts::api::{
    IndexFreshnessDto, IndexFreshnessStatusDto, RetrievalFallbackReasonDto,
};
use codestory_contracts::config_registry::{self, ObservedSetting};

pub(in crate::app) fn build_doctor_output(
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

    // Reported settings are observed through the registry so a secret-marked
    // value can never reach a doctor line, whatever this list grows to hold.
    let environment = [
        config_registry::EMBED_ALLOW_CPU_ENV,
        config_registry::STORED_VECTOR_ENCODING_ENV,
        config_registry::HYBRID_RETRIEVAL_ENABLED_ENV,
        config_registry::SEMANTIC_DOC_ALIAS_MODE_ENV,
    ]
    .into_iter()
    .map(|name| match config_registry::observe_env_setting(name) {
        ObservedSetting::Set(value) => {
            doctor_check(name, "ok", doctor_env_check_message(name, &value))
        }
        ObservedSetting::SetSecret => doctor_check(name, "ok", "set; value withheld".to_string()),
        ObservedSetting::Unset => {
            doctor_check(name, "info", "not set; using runtime defaults".to_string())
        }
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

pub(in crate::app::diagnostics) fn doctor_env_check_message(name: &str, value: &str) -> String {
    let trimmed = value.trim();
    if name.ends_with("_URL") || trimmed.contains("://") {
        return format!(
            "set to `{}`",
            embedding_config::redact_url_for_display(trimmed)
        );
    }
    format!("set to `{trimmed}`")
}

pub(in crate::app::diagnostics) fn redact_urls_in_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_url_token)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(in crate::app::diagnostics) fn redact_url_token(token: &str) -> String {
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

pub(in crate::app::diagnostics) fn index_freshness_check(
    freshness: &IndexFreshnessDto,
) -> DoctorCheckOutput {
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

pub(in crate::app) fn semantic_contract_check(
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
        // The stored contract is projected from the publication pointer (the
        // retrieval manifest), which records one opaque embedding runtime id
        // — the same identity space as the current contract's cache key —
        // plus a dimension. Per-doc profile/backend-label/doc-shape metadata
        // is not carried by the publication pointer: it is validated when the
        // generation is staged and folded into the sidecar input hash, so an
        // absent optional field is not evidence of drift. The cache key and
        // dimension are required publication evidence and keep failing closed
        // when they are absent.
        compare_carried_contract_field(
            &mut gaps,
            "embedding profile",
            stored.embedding_profile.as_deref(),
            current.profile.as_str(),
        );
        compare_carried_contract_field(
            &mut gaps,
            "embedding backend",
            stored.embedding_backend.as_deref(),
            current.backend.as_str(),
        );
        compare_contract_field(
            &mut gaps,
            "cache key",
            stored.cache_key.as_deref(),
            Some(current.cache_key.as_str()),
        );
        compare_carried_contract_field(
            &mut gaps,
            "semantic doc shape",
            stored.doc_shape.as_deref(),
            current.doc_shape.as_str(),
        );
        match (stored.dimension, current.dimension) {
            (Some(stored_dim), Some(current_dim)) if stored_dim != current_dim => {
                gaps.push(format!(
                    "embedding dimension mismatch: stored={stored_dim} current={current_dim}"
                ));
            }
            (None, Some(current_dim)) => {
                gaps.push(format!(
                    "embedding dimension missing from stored docs; current={current_dim}"
                ));
            }
            _ => {}
        }
    } else {
        gaps.push("current embedding config could not be resolved".to_string());
    }
    // The readiness projection already judged this publication against the
    // full manifest contract (schema, generation, degraded modes, policy
    // version). Honor its degraded verdict even when every field the
    // publication carries matches, so doctor never calls a stale or degraded
    // publication healthy.
    if retrieval.fallback_reason == Some(RetrievalFallbackReasonDto::DegradedRuntime) {
        gaps.push(
            "the published retrieval index is stale or degraded for the current contract"
                .to_string(),
        );
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

/// Flag a mismatch only when the stored contract actually carries the field.
///
/// Publication-pointer contracts legitimately omit per-doc metadata (profile,
/// backend label, doc shape); treating those omissions as gaps made doctor
/// report `semantic stale` forever on healthy manifest-published stores.
pub(in crate::app::diagnostics) fn compare_carried_contract_field(
    gaps: &mut Vec<String>,
    label: &str,
    stored: Option<&str>,
    current: &str,
) {
    if let Some(stored) = stored
        && stored != current
    {
        gaps.push(format!(
            "{label} mismatch: stored={stored} current={current}"
        ));
    }
}

pub(in crate::app::diagnostics) fn compare_contract_field(
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

pub(in crate::app::diagnostics) fn doctor_check(
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

pub(in crate::app::diagnostics) fn doctor_sidecar_check(
    sidecar: &RetrievalStatusOutput,
) -> DoctorCheckOutput {
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
pub(in crate::app) fn index_next_commands(
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
            IndexFreshnessStatusDto::NotChecked
                if crate::readiness::freshness_requires_refresh(freshness) =>
            {
                commands.push(format!(
                    "codestory-cli index --project {project} --refresh full"
                ));
                commands.push(format!(
                    "codestory-cli doctor --project {project} --format markdown"
                ));
                return commands;
            }
            IndexFreshnessStatusDto::Fresh | IndexFreshnessStatusDto::NotChecked => {}
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
