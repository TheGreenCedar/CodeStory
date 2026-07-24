use crate::symbol_query::{RetrievalFileRole, retrieval_file_role_from_path};
use codestory_contracts::api::{ApiError, FileCoverageDiagnosticDto, IndexedFileRoleDto};
use codestory_contracts::graph::FileCoverageReason;
use codestory_store::{
    FileInfo, IndexPublicationRecord, SourcePolicyExclusionPolicyIdentity,
    SourcePolicyExclusionRecord, Store,
};
use codestory_workspace::{
    OversizedSourceExclusionCandidate, RefreshExecutionPlan, RefreshMode, SourceIndexPolicy,
    WorkspaceInventoryOutcome, WorkspaceManifest, project_identity_v3,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub(crate) fn runtime_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(crate) fn normalize_path_key(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

pub(crate) fn indexed_file_role(path: &Path) -> IndexedFileRoleDto {
    path_role_from_key(&normalize_path_key(&path.to_string_lossy()))
}

pub(crate) fn file_coverage_reason(
    file: &FileInfo,
    errors_by_file: &HashMap<i64, Vec<FileCoverageReason>>,
    has_verified_content: bool,
) -> Option<FileCoverageReason> {
    if file.complete {
        return None;
    }
    if let Some(reason) = errors_by_file
        .get(&file.id)
        .and_then(|reasons| reasons.first())
    {
        return Some(*reason);
    }
    if !file.complete && file.indexed && has_verified_content {
        Some(FileCoverageReason::ParserPartial)
    } else {
        Some(FileCoverageReason::CollectorFailure)
    }
}

pub(crate) fn file_coverage_retryable(reason: FileCoverageReason) -> bool {
    matches!(
        reason,
        FileCoverageReason::SourceChanged
            | FileCoverageReason::DiscoveryIncomplete
            | FileCoverageReason::CollectorFailure
    )
}

pub(crate) fn file_coverage_detail(reason: FileCoverageReason) -> &'static str {
    match reason {
        FileCoverageReason::ParserPartial => {
            "stable verified source published with partial parser coverage"
        }
        FileCoverageReason::SourceChanged => "source changed while its projection was collected",
        FileCoverageReason::Unreadable => "source bytes could not be read and verified",
        FileCoverageReason::Malformed => {
            "verified UTF-8 source is malformed for its structural format"
        }
        FileCoverageReason::Binary => "source is binary or is not valid UTF-8",
        FileCoverageReason::Oversized => "source exceeds the configured indexing size limit",
        FileCoverageReason::DiscoveryIncomplete => {
            "workspace discovery could not prove a complete source inventory"
        }
        FileCoverageReason::CollectorFailure => {
            "a source collector or projection write failed before verification completed"
        }
    }
}

pub(crate) fn full_refresh_execution_plan_with_coverage(
    root: &Path,
    workspace: &WorkspaceManifest,
    policy: &SourceIndexPolicy,
) -> Result<(RefreshExecutionPlan, Vec<OversizedSourceExclusionCandidate>), ApiError> {
    let inventory = workspace
        .source_inventory_with_policy(policy)
        .map_err(|error| {
            ApiError::source_coverage_failure(
                "source_collector_failure",
                format!("Failed to collect the full source inventory: {error}"),
                vec![FileCoverageDiagnosticDto {
                    path: ".".to_string(),
                    reason: FileCoverageReason::CollectorFailure,
                    retryable: true,
                    verified_source: false,
                    projection_available: false,
                }],
            )
        })?;
    if inventory.outcome != WorkspaceInventoryOutcome::Complete {
        let reason = if inventory.outcome == WorkspaceInventoryOutcome::Unreadable {
            FileCoverageReason::Unreadable
        } else {
            FileCoverageReason::DiscoveryIncomplete
        };
        let mut coverage_gaps = inventory
            .issues
            .iter()
            .map(|issue| FileCoverageDiagnosticDto {
                path: runtime_relative_path(root, &issue.path),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source: false,
                projection_available: false,
            })
            .collect::<Vec<_>>();
        if coverage_gaps.is_empty() {
            coverage_gaps.push(FileCoverageDiagnosticDto {
                path: ".".to_string(),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source: false,
                projection_available: false,
            });
        }
        return Err(ApiError::source_coverage_failure(
            match reason {
                FileCoverageReason::Unreadable => "source_unreadable",
                _ => "source_discovery_incomplete",
            },
            format!(
                "Effective refresh mode `full` requires a complete source inventory; discovery was {:?}.",
                inventory.outcome
            ),
            coverage_gaps,
        ));
    }
    Ok((
        RefreshExecutionPlan {
            mode: RefreshMode::FullRefresh,
            files_to_index: inventory.files,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        inventory.policy_exclusions,
    ))
}

pub(crate) fn publish_source_policy_exclusions(
    storage: &mut Store,
    root: &Path,
    publication: &IndexPublicationRecord,
    exclusions: &[OversizedSourceExclusionCandidate],
    policy: &SourceIndexPolicy,
) -> Result<(), ApiError> {
    let identity = project_identity_v3(root);
    storage
        .publish_source_policy_exclusion_generation(
            publication,
            &identity.project_id,
            &identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &policy.policy_version,
                policy.byte_cap,
                policy.structural_unit_cap,
            ),
            exclusions,
        )
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to publish complete source policy exclusions: {error}"
            ))
        })?;
    Ok(())
}

pub(crate) fn source_policy_exclusion_candidate(
    record: &SourcePolicyExclusionRecord,
) -> OversizedSourceExclusionCandidate {
    OversizedSourceExclusionCandidate {
        normalized_path: record.normalized_path.clone(),
        content_hash: record.content_hash.clone(),
        observed_size: record.observed_size,
        observed_unit_count: record.observed_unit_count,
        policy_version: record.policy_version.clone(),
        byte_cap: record.byte_cap,
        structural_unit_cap: record.structural_unit_cap,
    }
}

pub(crate) fn revalidate_source_policy_exclusions(
    workspace: &WorkspaceManifest,
    exclusions: &[OversizedSourceExclusionCandidate],
    policy: &SourceIndexPolicy,
) -> Result<Vec<OversizedSourceExclusionCandidate>, ApiError> {
    workspace
        .revalidate_source_policy_exclusions(exclusions, policy)
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!(
                    "Source policy exclusions changed before publication; the candidate core was discarded: {error}"
                ),
            )
        })
}

pub(crate) fn validate_source_policy_exclusions(
    storage: &Store,
    root: &Path,
    publication: &IndexPublicationRecord,
    policy: &SourceIndexPolicy,
) -> Result<(), ApiError> {
    let identity = project_identity_v3(root);
    storage
        .validate_source_policy_exclusion_publication(
            publication,
            &identity.project_id,
            &identity.workspace_id,
            SourcePolicyExclusionPolicyIdentity::new(
                &policy.policy_version,
                policy.byte_cap,
                policy.structural_unit_cap,
            ),
        )
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!("Source policy exclusion publication is incomplete or stale: {error}"),
            )
        })?;
    Ok(())
}

pub(crate) fn validate_structural_text_units(
    storage: &Store,
    publication: &IndexPublicationRecord,
) -> Result<(), ApiError> {
    storage
        .validate_structural_text_unit_publication(publication)
        .map_err(|error| {
            ApiError::new(
                "source_verification_failed",
                format!("Structural text unit publication is incomplete or stale: {error}"),
            )
        })?;
    Ok(())
}

pub(crate) fn stored_file_coverage_diagnostics(
    root: &Path,
    storage: &Store,
) -> Result<Vec<FileCoverageDiagnosticDto>, ApiError> {
    let files = storage.get_files().map_err(|error| {
        ApiError::internal(format!("Failed to load staged file coverage: {error}"))
    })?;
    let verified_file_ids = storage
        .files()
        .inventory()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load staged verified source identities: {error}"
            ))
        })?
        .into_iter()
        .filter_map(|file| file.content_hash.map(|_| file.id))
        .collect::<HashSet<_>>();
    let structural_projection_file_ids = storage
        .get_structural_text_projection_file_ids()
        .map_err(|error| {
            ApiError::internal(format!(
                "Failed to load staged structural projection identities: {error}"
            ))
        })?
        .into_iter()
        .collect::<HashSet<_>>();
    let mut dedicated_openapi_projection_file_ids = HashSet::new();
    for file in &files {
        if file.language == "openapi"
            && verified_file_ids.contains(&file.id)
            && storage
                .has_file_owned_openapi_endpoint_projection(file.id)
                .map_err(|error| {
                    ApiError::internal(format!(
                        "Failed to verify staged OpenAPI projection identity for {}: {error}",
                        runtime_relative_path(root, &file.path)
                    ))
                })?
        {
            dedicated_openapi_projection_file_ids.insert(file.id);
        }
    }
    let mut errors_by_file = HashMap::<i64, Vec<FileCoverageReason>>::new();
    for error in storage.get_errors(None).map_err(|error| {
        ApiError::internal(format!("Failed to load staged file errors: {error}"))
    })? {
        if let Some(file_id) = error.file_id {
            errors_by_file.entry(file_id.0).or_default().push(
                error
                    .coverage_reason
                    .unwrap_or(FileCoverageReason::CollectorFailure),
            );
        }
    }
    Ok(files
        .iter()
        .filter_map(|file| {
            let verified_source = verified_file_ids.contains(&file.id);
            let dedicated_openapi_source = file.language == "openapi"
                && verified_source
                && dedicated_openapi_projection_file_ids.contains(&file.id);
            let structural_projection_verified = dedicated_openapi_source
                || !codestory_indexer::structural::is_structural_candidate_path(&file.path)
                || (verified_source && structural_projection_file_ids.contains(&file.id));
            let reason = if file.complete && !structural_projection_verified {
                Some(FileCoverageReason::CollectorFailure)
            } else {
                file_coverage_reason(file, &errors_by_file, verified_source)
            };
            reason.map(|reason| FileCoverageDiagnosticDto {
                path: runtime_relative_path(root, &file.path),
                reason,
                retryable: file_coverage_retryable(reason),
                verified_source,
                projection_available: file.indexed
                    && verified_source
                    && structural_projection_verified,
            })
        })
        .collect())
}

pub(crate) fn source_coverage_failure_code(
    coverage_gaps: &[FileCoverageDiagnosticDto],
) -> &'static str {
    let Some(first) = coverage_gaps.first().map(|entry| entry.reason) else {
        return "source_verification_failed";
    };
    if coverage_gaps.iter().any(|entry| entry.reason != first) {
        return "source_verification_failed";
    }
    match first {
        FileCoverageReason::ParserPartial => "source_verification_failed",
        FileCoverageReason::SourceChanged => "source_changed",
        FileCoverageReason::Unreadable => "source_unreadable",
        FileCoverageReason::Malformed => "source_malformed",
        FileCoverageReason::Binary => "source_binary",
        FileCoverageReason::Oversized => "source_oversized",
        FileCoverageReason::DiscoveryIncomplete => "source_discovery_incomplete",
        FileCoverageReason::CollectorFailure => "source_collector_failure",
    }
}

pub(crate) fn path_role_from_key(path: &str) -> IndexedFileRoleDto {
    match retrieval_file_role_from_path(path) {
        RetrievalFileRole::Test => IndexedFileRoleDto::Test,
        RetrievalFileRole::Generated => IndexedFileRoleDto::Generated,
        RetrievalFileRole::Vendor => IndexedFileRoleDto::Vendor,
        RetrievalFileRole::Source | RetrievalFileRole::Docs | RetrievalFileRole::Benchmark => {
            IndexedFileRoleDto::Source
        }
    }
}
