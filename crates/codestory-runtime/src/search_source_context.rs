//! Runtime boundary for bounded, confined current-source context.
//!
//! Caller supplies actual path/physical-row observations and an explicit project
//! and cwd. This operation does not activate indexing or consume a question.

use codestory_contracts::source_context::{
    SOURCE_CONTEXT_BOUNDS, SourceContextCoverage, SourceContextDecline, SourceContextDeclineReason,
    SourceContextRequest, SourceContextResponse, SourceContextStatus,
};
use codestory_indexer::CancellationToken;

#[cfg(unix)]
mod io;

pub fn search_source_context(
    request: &SourceContextRequest,
    cancellation: &CancellationToken,
) -> SourceContextResponse {
    let mut response = SourceContextResponse {
        status: SourceContextStatus::RawFallback,
        units: Vec::new(),
        declines: Vec::new(),
        coverage: SourceContextCoverage {
            observations_received: request.observations.len(),
            ..SourceContextCoverage::default()
        },
        bounds: SOURCE_CONTEXT_BOUNDS,
        current_post_search_snapshot: false,
        exhaustive_search: false,
        atomic_publication: false,
        indexed_proof: false,
    };
    #[cfg(unix)]
    {
        let mut remaining_reads = SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes;
        let outcome = collect(request, cancellation, &mut remaining_reads, &mut response);
        response.coverage.source_read_bytes =
            SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes - remaining_reads;
        if let Err(reason) = outcome {
            response.units.clear();
            decline(&mut response, None, reason);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cancellation;
        decline(
            &mut response,
            None,
            SourceContextDeclineReason::UnsupportedPlatform,
        );
    }
    if !response.units.is_empty() {
        response.status = SourceContextStatus::BoundedCurrentSource;
        response.current_post_search_snapshot = true;
    } else if response.declines.is_empty() {
        decline(
            &mut response,
            None,
            SourceContextDeclineReason::NoEnclosingUnit,
        );
    }
    response
        .declines
        .sort_by_key(|d| d.observation_index.unwrap_or(usize::MAX));
    response
}

fn decline(
    response: &mut SourceContextResponse,
    observation_index: Option<usize>,
    reason: SourceContextDeclineReason,
) {
    response.declines.push(SourceContextDecline {
        observation_index,
        reason,
    });
}

#[cfg(unix)]
fn spend(
    budget: &mut codestory_indexer::source_units::SourceUnitBudget,
    cancellation: &CancellationToken,
    amount: usize,
) -> Result<(), SourceContextDeclineReason> {
    if cancellation.is_cancelled() {
        return Err(SourceContextDeclineReason::Cancelled);
    }
    if amount > budget.remaining_work {
        budget.remaining_work = 0;
        return Err(SourceContextDeclineReason::WorkBudgetExceeded);
    }
    budget.remaining_work -= amount;
    Ok(())
}

#[cfg(unix)]
struct FileInput {
    relative: std::path::PathBuf,
    observations: Vec<usize>,
    loaded: Option<LoadedSource>,
}

#[cfg(unix)]
struct LoadedSource {
    read: io::SourceRead,
    sha256: String,
}

#[cfg(unix)]
fn collect(
    request: &SourceContextRequest,
    cancellation: &CancellationToken,
    remaining_reads: &mut usize,
    response: &mut SourceContextResponse,
) -> Result<(), SourceContextDeclineReason> {
    use codestory_contracts::source_context::{
        SourceContextDeclineReason as Reason, SourceContextUnit,
    };
    use codestory_indexer::source_units::{
        SourceUnitBudget, SourceUnitError, SourceUnitGap, SourceUnitSelection, compile_source_units,
    };
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    let mut budget = SourceUnitBudget {
        remaining_source_bytes: SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes,
        remaining_work: SOURCE_CONTEXT_BOUNDS.work_units,
        max_depth: SOURCE_CONTEXT_BOUNDS.max_depth,
    };
    spend(
        &mut budget,
        cancellation,
        request.project.len().saturating_add(request.cwd.len()),
    )?;
    let root = io::Directory::pin(Path::new(&request.project), cancellation)?;
    let cwd = root.cwd(Path::new(&request.cwd), cancellation)?;
    let considered = request.observations.len().min(SOURCE_CONTEXT_BOUNDS.hits);
    response.coverage.observations_considered = considered;
    if request.observations.len() > considered {
        decline(response, Some(considered), Reason::HitLimit);
    }
    let mut inputs: Vec<FileInput> = Vec::new();
    let mut paths = BTreeMap::new();
    for (index, observation) in request.observations[..considered].iter().enumerate() {
        spend(
            &mut budget,
            cancellation,
            observation.path.len().saturating_add(1),
        )?;
        let relative = match root.source_path(&cwd, Path::new(&observation.path)) {
            Ok(path) => path,
            Err(reason) => {
                decline(response, Some(index), reason);
                continue;
            }
        };
        if !matches!(
            relative
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("py" | "rs" | "go")
        ) {
            decline(response, Some(index), Reason::UnsupportedSource);
            continue;
        }
        let file_index = if let Some(&file_index) = paths.get(&relative) {
            file_index
        } else {
            if inputs.len() >= SOURCE_CONTEXT_BOUNDS.files {
                decline(response, Some(index), Reason::FileLimit);
                continue;
            }
            let file_index = inputs.len();
            paths.insert(relative.clone(), file_index);
            inputs.push(FileInput {
                relative,
                observations: Vec::new(),
                loaded: None,
            });
            file_index
        };
        inputs[file_index].observations.push(index);
    }
    let mut selected: Vec<Option<(usize, SourceUnitSelection)>> = vec![None; considered];
    for (file_index, input) in inputs.iter_mut().enumerate() {
        spend(&mut budget, cancellation, 1)?;
        let read = match root.read(
            &input.relative,
            None,
            SOURCE_CONTEXT_BOUNDS.per_file_bytes,
            remaining_reads,
            cancellation,
        ) {
            Ok(read) => read,
            Err(reason) => {
                if matches!(
                    reason,
                    Reason::SourceDrift | Reason::ReadBudgetExceeded | Reason::Cancelled
                ) {
                    return Err(reason);
                }
                for &index in &input.observations {
                    decline(response, Some(index), reason);
                }
                continue;
            }
        };
        response.coverage.files_read += 1;
        spend(&mut budget, cancellation, read.bytes.len())?;
        let text = match std::str::from_utf8(&read.bytes) {
            Ok(text) => text,
            Err(_) => {
                for &index in &input.observations {
                    decline(response, Some(index), Reason::InvalidUtf8);
                }
                continue;
            }
        };
        spend(&mut budget, cancellation, read.bytes.len())?;
        let physical = text.split_inclusive('\n').collect::<Vec<_>>();
        let mut lines = Vec::new();
        for &index in &input.observations {
            let observation = &request.observations[index];
            spend(
                &mut budget,
                cancellation,
                observation.text.len().saturating_add(1),
            )?;
            let row = observation
                .physical_line
                .checked_sub(1)
                .and_then(|line| physical.get(line));
            if row.is_none_or(|row| row.strip_suffix('\n').unwrap_or(row) != observation.text) {
                decline(response, Some(index), Reason::ObservedRowMismatch);
                return Err(Reason::ObservedRowMismatch);
            }
            lines.push(observation.physical_line);
        }
        spend(&mut budget, cancellation, read.bytes.len())?;
        let sha256 = format!("{:x}", Sha256::digest(&read.bytes));
        response.coverage.files_compiled += 1;
        let extension = input
            .relative
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or(Reason::UnsupportedSource)?;
        match compile_source_units(extension, text, &lines, &mut budget, cancellation) {
            Ok(selections) => {
                for (&index, selection) in input.observations.iter().zip(selections) {
                    selected[index] = Some((file_index, selection));
                }
            }
            Err(error) => {
                let reason = match error {
                    SourceUnitError::Cancelled => Reason::Cancelled,
                    SourceUnitError::WorkBudgetExceeded => Reason::WorkBudgetExceeded,
                    SourceUnitError::DepthExceeded => Reason::DepthExceeded,
                    SourceUnitError::SourceByteBudgetExceeded => Reason::ReadBudgetExceeded,
                    SourceUnitError::MalformedSource => Reason::MalformedSource,
                    SourceUnitError::ParserUnavailable | SourceUnitError::UnsupportedSyntax => {
                        Reason::UnsupportedSource
                    }
                };
                if matches!(
                    reason,
                    Reason::Cancelled
                        | Reason::WorkBudgetExceeded
                        | Reason::DepthExceeded
                        | Reason::ReadBudgetExceeded
                ) {
                    return Err(reason);
                }
                for &index in &input.observations {
                    decline(response, Some(index), reason);
                }
            }
        }
        input.loaded = Some(LoadedSource { read, sha256 });
    }
    #[cfg(test)]
    source_context_test_stage(SourceContextTestStage::BeforeFinalRecheck);
    for input in &inputs {
        let Some(loaded) = &input.loaded else {
            continue;
        };
        let again = root
            .read(
                &input.relative,
                Some(&loaded.read.identity),
                SOURCE_CONTEXT_BOUNDS.per_file_bytes,
                remaining_reads,
                cancellation,
            )
            .map_err(|reason| {
                if matches!(reason, Reason::Cancelled | Reason::ReadBudgetExceeded) {
                    reason
                } else {
                    Reason::SourceDrift
                }
            })?;
        spend(&mut budget, cancellation, again.bytes.len())?;
        if again.bytes != loaded.read.bytes {
            return Err(Reason::SourceDrift);
        }
    }
    root.validate_root(cancellation)?;
    let mut seen = BTreeSet::new();
    for (index, selection) in selected.into_iter().enumerate() {
        spend(&mut budget, cancellation, 1)?;
        let Some((file_index, selection)) = selection else {
            continue;
        };
        for gap in selection.gaps {
            let reason = match gap {
                SourceUnitGap::InvalidMatchLine => Reason::InvalidMatchLine,
                SourceUnitGap::NoEnclosingUnit => Reason::NoEnclosingUnit,
                SourceUnitGap::InlineAncestorHeader => Reason::InlineAncestorHeader,
                SourceUnitGap::AncestorHeaderTooLarge => Reason::AncestorHeaderTooLarge,
                SourceUnitGap::AncestorDepthExceeded => Reason::AncestorDepthExceeded,
            };
            decline(response, Some(index), reason);
        }
        let input = &inputs[file_index];
        let loaded = input.loaded.as_ref().ok_or(Reason::UnreadableSource)?;
        for unit in selection.units {
            spend(&mut budget, cancellation, 1)?;
            let key = (file_index, unit.start_line, unit.end_line);
            if seen.contains(&key) {
                continue;
            }
            let body = loaded
                .read
                .bytes
                .get(unit.start_byte..unit.end_byte)
                .ok_or(Reason::MalformedSource)?;
            if body.len() > SOURCE_CONTEXT_BOUNDS.display_source_bytes {
                decline(response, Some(index), Reason::UnitTooLarge);
                continue;
            }
            spend(&mut budget, cancellation, body.len().saturating_mul(2))?;
            let body_text = std::str::from_utf8(body).map_err(|_| Reason::InvalidUtf8)?;
            seen.insert(key);
            response.units.push(SourceContextUnit {
                path: input
                    .relative
                    .to_str()
                    .ok_or(Reason::InvalidPath)?
                    .to_owned(),
                source_sha256: loaded.sha256.clone(),
                start_line: unit.start_line,
                end_line: unit.end_line,
                start_byte: unit.start_byte,
                end_byte: unit.end_byte,
                kind: unit.kind,
                body: body_text.to_owned(),
                body_sha256: format!("{:x}", Sha256::digest(body)),
            });
        }
    }
    spend(&mut budget, cancellation, 0)?;
    Ok(())
}

// Private deterministic mutation/observation seam; absent from product builds.
// These run at the actual pinned open/read/recheck boundary, not a mock.
#[cfg(all(test, unix))]
enum SourceContextTestStage<'a> {
    BeforeOpen {
        relative: &'a std::path::Path,
    },
    BeforeRead {
        file: &'a std::fs::File,
    },
    AfterRead {
        file: &'a std::fs::File,
        bytes: usize,
    },
    BeforeFinalRecheck,
}

#[cfg(all(test, unix))]
type SourceContextTestHook = Box<dyn for<'a> FnMut(SourceContextTestStage<'a>)>;

#[cfg(all(test, unix))]
thread_local! {
    static SOURCE_CONTEXT_TEST_HOOK: std::cell::RefCell<Option<SourceContextTestHook>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, unix))]
fn source_context_test_stage(stage: SourceContextTestStage<'_>) {
    SOURCE_CONTEXT_TEST_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(stage);
        }
    });
}

#[cfg(all(test, unix))]
#[path = "search_source_context_tests.rs"]
mod tests;

#[cfg(all(test, not(unix)))]
mod unsupported_platform_tests {
    use super::*;

    #[test]
    fn unsupported_platform_returns_no_source_extras() {
        let request = SourceContextRequest {
            project: String::new(),
            cwd: String::new(),
            observations: Vec::new(),
        };
        let response = search_source_context(&request, &CancellationToken::new());
        assert_eq!(response.status, SourceContextStatus::RawFallback);
        assert!(response.units.is_empty());
        assert!(
            response
                .declines
                .iter()
                .any(|d| d.reason == SourceContextDeclineReason::UnsupportedPlatform)
        );
    }
}
