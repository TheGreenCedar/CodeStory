//! What the published index knows about specific source files.
//!
//! This exists because an incompletely indexed file was invisible to the agent:
//! nothing under `agent/` referenced a coverage reason or an exclusion, so a
//! packet could rest a proof-bearing claim on a file the index had deliberately
//! refused and still report `Sufficient`.
//!
//! The one contract worth stating up front: **this maps, it never filters.**
//! Every requested path gets exactly one observation, including when the lookup
//! fails. A producer that dropped unknown paths would be indistinguishable from
//! one reporting them as covered, which is the defect itself moved one layer up.

use crate::AppController;
use crate::index_coverage::stored_file_coverage_diagnostics;
use codestory_contracts::api::{
    SourceCoverageNotEstablishedCauseDto, SourceCoverageObservationDto, SourceCoverageStatusDto,
};
use codestory_workspace::same_workspace_path;
use std::path::{Path, PathBuf};

/// Observe coverage for `paths`, one observation each, in the order given.
///
/// Paths may be absolute or workspace-relative; both are resolved against the
/// project root before comparison.
pub(crate) fn observe_source_coverage(
    controller: &AppController,
    paths: &[String],
) -> Vec<SourceCoverageObservationDto> {
    let mut deduped: Vec<&String> = Vec::new();
    for path in paths {
        if !deduped.iter().any(|seen| seen.as_str() == path.as_str()) {
            deduped.push(path);
        }
    }
    if deduped.is_empty() {
        return Vec::new();
    }

    let Ok(project_root) = controller.require_project_root() else {
        return not_established(
            &deduped,
            SourceCoverageNotEstablishedCauseDto::LookupUnavailable,
        );
    };
    let Ok(storage) = controller.open_storage_read_only() else {
        return not_established(
            &deduped,
            SourceCoverageNotEstablishedCauseDto::LookupUnavailable,
        );
    };
    let exclusions = match storage.get_source_policy_exclusions() {
        Ok(exclusions) => exclusions,
        Err(_) => {
            return not_established(
                &deduped,
                SourceCoverageNotEstablishedCauseDto::LookupUnavailable,
            );
        }
    };

    // `ParserPartial` is the one coverage reason that survives publication —
    // both refresh gates refuse to commit on any other — so it is exactly the
    // defect a served packet can rest on, and reporting such a file `Indexed`
    // would contradict this contract's own definition of the word.
    let diagnostics = stored_file_coverage_diagnostics(&project_root, &storage).unwrap_or_default();
    let incomplete: Vec<(PathBuf, codestory_contracts::graph::FileCoverageReason)> = diagnostics
        .iter()
        .map(|diagnostic| (project_root.join(&diagnostic.path), diagnostic.reason))
        .collect();

    // Resolved once, not per requested path: the comparison is by path identity
    // rather than by string, so each side has to be made absolute first.
    let excluded: Vec<(PathBuf, u64, u64)> = exclusions
        .iter()
        .map(|record| {
            (
                project_root.join(&record.normalized_path),
                record.observed_size,
                record.byte_cap,
            )
        })
        .collect();

    deduped
        .into_iter()
        .map(|path| {
            let absolute = absolute_against(&project_root, path);
            match excluded
                .iter()
                .find(|(excluded_path, _, _)| same_workspace_path(excluded_path, &absolute))
            {
                Some((_, observed_size, byte_cap)) => SourceCoverageObservationDto {
                    path: path.clone(),
                    status: SourceCoverageStatusDto::PolicyExcluded,
                    reason: None,
                    not_established_cause: None,
                    observed_size: Some(*observed_size),
                    byte_cap: Some(*byte_cap),
                },
                None => match incomplete
                    .iter()
                    .find(|(defect_path, _)| same_workspace_path(defect_path, &absolute))
                {
                    Some((_, reason)) => SourceCoverageObservationDto {
                        path: path.clone(),
                        status: SourceCoverageStatusDto::Incomplete,
                        reason: Some(*reason),
                        not_established_cause: None,
                        observed_size: None,
                        byte_cap: None,
                    },
                    None => SourceCoverageObservationDto {
                        path: path.clone(),
                        status: SourceCoverageStatusDto::Indexed,
                        reason: None,
                        not_established_cause: None,
                        observed_size: None,
                        byte_cap: None,
                    },
                },
            }
        })
        .collect()
}

fn absolute_against(project_root: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        project_root.join(candidate)
    }
}

fn not_established(
    paths: &[&String],
    cause: SourceCoverageNotEstablishedCauseDto,
) -> Vec<SourceCoverageObservationDto> {
    paths
        .iter()
        .map(|path| SourceCoverageObservationDto {
            path: (*path).clone(),
            status: SourceCoverageStatusDto::NotEstablished,
            reason: None,
            not_established_cause: Some(cause),
            observed_size: None,
            byte_cap: None,
        })
        .collect()
}
