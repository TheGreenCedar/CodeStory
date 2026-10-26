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
        selection: None,
    };
    #[cfg(unix)]
    {
        let mut remaining_reads = SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes;
        let outcome = collect(request, cancellation, &mut remaining_reads, &mut response);
        response.coverage.source_read_bytes =
            SOURCE_CONTEXT_BOUNDS.aggregate_read_bytes - remaining_reads;
        if let Err(reason) = outcome {
            response.units.clear();
            response.selection = None;
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

/// Standalone literal body plus the existing CLI framing/separator cost.
/// The CLI remains the render authority; its executable equality test pins this
/// cost to the actual formatter (including JSON path escaping and UTF-8).
pub fn source_context_unit_cost(
    unit: &codestory_contracts::source_context::SourceContextUnit,
) -> usize {
    let path = serde_json::to_string(&unit.path).unwrap_or_default();
    let header = format!(
        "\n[source {path}; sha256={}; physical_lines={}-{}; {}; exact_bytes={}]\n",
        unit.source_sha256,
        unit.start_line,
        unit.end_line,
        unit.kind,
        unit.body.len()
    );
    header.len() + unit.body.len() + usize::from(!unit.body.ends_with('\n'))
}

struct QueryPattern<'a> {
    branches: Vec<&'a str>,
}

impl<'a> QueryPattern<'a> {
    fn parse(query: &'a codestory_contracts::source_context::SourceContextQuery) -> Option<Self> {
        let pattern = query.pattern.as_str();
        if pattern.is_empty() || pattern.len() > 256 || !pattern.is_ascii() {
            return None;
        }
        let mut branches = Vec::new();
        for branch in pattern.split('|') {
            if branch.is_empty()
                || branch.bytes().filter(|&b| b == b'.').count() > 1
                || branch
                    .bytes()
                    .any(|b| !(b' '..=b'~').contains(&b) || b"\\^$*+?{}[]()".contains(&b))
            {
                return None;
            }
            if !branches.contains(&branch) {
                branches.push(branch);
            }
            if branches.len() > 16 {
                return None;
            }
        }
        (branches.len() >= 2).then_some(Self { branches })
    }

    fn bits(
        &self,
        text: &str,
        budget: &mut codestory_indexer::source_units::SourceUnitBudget,
        cancellation: &CancellationToken,
    ) -> Result<u16, SourceContextDeclineReason> {
        let mut bits = 0u16;
        for (index, branch) in self.branches.iter().enumerate() {
            for (start, _) in text.char_indices() {
                spend(budget, cancellation, 1)?;
                let mut chars = text[start..].chars();
                let mut matched = true;
                for wanted in branch.bytes() {
                    spend(budget, cancellation, 1)?;
                    let actual = chars.next();
                    if !actual.is_some_and(|c| {
                        if wanted == b'.' {
                            c != '\n'
                        } else {
                            c == char::from(wanted)
                        }
                    }) {
                        matched = false;
                        break;
                    }
                }
                if matched {
                    bits |= 1u16 << index;
                    break;
                }
            }
        }
        spend(budget, cancellation, 0)?;
        Ok(bits)
    }
}

#[cfg(test)]
fn query_branch_bits(
    query: &codestory_contracts::source_context::SourceContextQuery,
    text: &str,
    budget: &mut codestory_indexer::source_units::SourceUnitBudget,
    cancellation: &CancellationToken,
) -> Result<Option<u16>, SourceContextDeclineReason> {
    spend(budget, cancellation, query.pattern.len().min(257))?;
    let Some(pattern) = QueryPattern::parse(query) else {
        return Ok(None);
    };
    pattern.bits(text, budget, cancellation).map(Some)
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
    // Unknown/unsupported query metadata preserves the ordinary prefix path.
    let query = if let Some(query) = request.query.as_ref() {
        spend(&mut budget, cancellation, query.pattern.len().min(257))?;
        QueryPattern::parse(query)
    } else {
        None
    };
    let count = request.observations.len();
    let considered = count.min(SOURCE_CONTEXT_BOUNDS.hits);
    if query.is_some() {
        // Covers bounded sampling and first-omission lookup, including their
        // index comparisons. It shares the parse/match allowance below.
        spend(
            &mut budget,
            cancellation,
            considered.saturating_mul(considered + 2),
        )?;
    }
    let observation_indices: Vec<usize> = if query.is_some() && count > considered {
        (0..considered)
            .map(|i| ((i as u128 * (count - 1) as u128) / (considered - 1) as u128) as usize)
            .collect()
    } else {
        (0..considered).collect()
    };
    response.coverage.observations_considered = considered;
    if count > considered {
        let omitted = (0..=considered)
            .find(|i| !observation_indices.contains(i))
            .unwrap_or(considered);
        decline(response, Some(omitted), Reason::HitLimit);
    }
    let mut inputs: Vec<FileInput> = Vec::new();
    let mut paths = BTreeMap::new();
    for &index in &observation_indices {
        let observation = &request.observations[index];
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
            Some("py" | "rs")
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
    let mut selected: BTreeMap<usize, (usize, SourceUnitSelection)> = BTreeMap::new();
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
                    selected.insert(index, (file_index, selection));
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
    if let Some(query) = query {
        return ranked_units(
            request,
            &inputs,
            selected,
            observation_indices,
            &query,
            &mut budget,
            cancellation,
            response,
        );
    }
    let mut seen = BTreeSet::new();
    for (index, (file_index, selection)) in selected {
        spend(&mut budget, cancellation, 1)?;
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

#[cfg(unix)]
fn ranked_units(
    request: &SourceContextRequest,
    inputs: &[FileInput],
    selected: std::collections::BTreeMap<
        usize,
        (usize, codestory_indexer::source_units::SourceUnitSelection),
    >,
    observation_indices: Vec<usize>,
    query: &QueryPattern<'_>,
    budget: &mut codestory_indexer::source_units::SourceUnitBudget,
    cancellation: &CancellationToken,
    response: &mut SourceContextResponse,
) -> Result<(), SourceContextDeclineReason> {
    use codestory_contracts::source_context::{
        SourceContextBundle, SourceContextDeclineReason as Reason, SourceContextSelection,
        SourceContextUnit,
    };
    use codestory_indexer::source_units::SourceUnitGap;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    // All rows, bytes and native identities have passed the same final source
    // recheck as ordinary context before any feature can affect selection.
    let mut row_bits = BTreeMap::new();
    for input in inputs {
        if input.loaded.is_none() {
            continue;
        }
        for &index in &input.observations {
            row_bits.insert(
                index,
                query.bits(&request.observations[index].text, budget, cancellation)?,
            );
        }
    }
    struct Bundle {
        primary: usize,
        headers: Vec<usize>,
        order: Vec<usize>,
        bits: u16,
        first: usize,
        cost: usize,
    }
    let mut units: Vec<Option<SourceContextUnit>> = Vec::new();
    let mut unit_ids = BTreeMap::new();
    let mut bundles: Vec<Bundle> = Vec::new();
    let mut bundle_ids = BTreeMap::new();
    for (index, (file_index, selection)) in selected {
        spend(budget, cancellation, 1)?;
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
        // The compiler preserves physical order: partial ancestor headers can
        // precede the complete owner. Classify roles explicitly, not by offset.
        let Some(primary_position) = selection
            .units
            .iter()
            .position(|unit| unit.kind != "ancestor_header_context")
        else {
            continue;
        };
        let primary = &selection.units[primary_position];
        if primary.end_byte.saturating_sub(primary.start_byte)
            > SOURCE_CONTEXT_BOUNDS.display_source_bytes
        {
            decline(response, Some(index), Reason::UnitTooLarge);
            continue;
        }
        let first_line = primary.start_line;
        let last_line = primary.end_line;
        let input = &inputs[file_index];
        let loaded = input.loaded.as_ref().ok_or(Reason::UnreadableSource)?;
        let mut ids = Vec::new();
        let mut headers = Vec::new();
        let mut primary_id = None;
        for (position, unit) in selection.units.into_iter().enumerate() {
            spend(budget, cancellation, 1)?;
            let is_primary = position == primary_position;
            if !is_primary && unit.kind != "ancestor_header_context" {
                return Err(Reason::MalformedSource);
            }
            let key = (
                file_index,
                unit.start_byte,
                unit.end_byte,
                unit.kind.clone(),
            );
            let id = if let Some(&id) = unit_ids.get(&key) {
                id
            } else {
                let body = loaded
                    .read
                    .bytes
                    .get(unit.start_byte..unit.end_byte)
                    .ok_or(Reason::MalformedSource)?;
                if body.len() > SOURCE_CONTEXT_BOUNDS.display_source_bytes {
                    return Err(Reason::UnitTooLarge);
                }
                spend(
                    budget,
                    cancellation,
                    body.len()
                        .saturating_mul(2)
                        .saturating_add(input.relative.as_os_str().len()),
                )?;
                let body_text = std::str::from_utf8(body).map_err(|_| Reason::InvalidUtf8)?;
                let id = units.len();
                units.push(Some(SourceContextUnit {
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
                }));
                unit_ids.insert(key, id);
                id
            };
            if !ids.contains(&id) {
                ids.push(id);
            }
            if is_primary {
                primary_id = Some(id);
            } else if !headers.contains(&id) {
                headers.push(id);
            }
        }
        let primary = primary_id.ok_or(Reason::MalformedSource)?;
        let mut bits = 0;
        for &row_index in &input.observations {
            spend(budget, cancellation, 1)?;
            let row = &request.observations[row_index];
            if (first_line..=last_line).contains(&row.physical_line) {
                bits |= row_bits.get(&row_index).copied().unwrap_or(0);
            }
        }
        if let Some(&id) = bundle_ids.get(&primary) {
            let bundle: &mut Bundle = &mut bundles[id];
            bundle.bits |= bits;
        } else {
            let mut cost = 0usize;
            for &id in &ids {
                let unit = units[id].as_ref().ok_or(Reason::MalformedSource)?;
                spend(
                    budget,
                    cancellation,
                    unit.path
                        .len()
                        .saturating_add(unit.kind.len())
                        .saturating_add(1),
                )?;
                cost = cost.saturating_add(source_context_unit_cost(unit));
            }
            bundle_ids.insert(primary, bundles.len());
            bundles.push(Bundle {
                primary,
                headers,
                order: ids,
                bits,
                first: index,
                cost,
            });
        }
    }
    // At most64 bundles. Charge a conservative comparison allowance before
    // sorting numeric keys; no body text or unadmitted rows enter this ordering.
    spend(
        budget,
        cancellation,
        bundles.len().saturating_mul(bundles.len()),
    )?;
    bundles.sort_by_key(|bundle| {
        (
            std::cmp::Reverse(bundle.bits.count_ones()),
            bundle.cost,
            bundle.first,
        )
    });
    let mut remapped = vec![None; units.len()];
    let mut admitted = Vec::new();
    for bundle in bundles {
        // Keep each bundle's compiler order, including headers-first output,
        // while its typed dependency carrier identifies the primary explicitly.
        for old in bundle.order {
            spend(budget, cancellation, 1)?;
            if remapped[old].is_none() {
                let id = response.units.len();
                response
                    .units
                    .push(units[old].take().ok_or(Reason::MalformedSource)?);
                remapped[old] = Some(id);
            }
        }
        let primary_unit_index = remapped[bundle.primary].ok_or(Reason::MalformedSource)?;
        let required_header_indices = bundle
            .headers
            .into_iter()
            .map(|id| remapped[id].ok_or(Reason::MalformedSource))
            .collect::<Result<Vec<_>, _>>()?;
        admitted.push(SourceContextBundle {
            primary_unit_index,
            required_header_indices,
        });
    }
    spend(budget, cancellation, 0)?;
    response.selection = Some(SourceContextSelection {
        observation_indices,
        bundles: admitted,
    });
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
            query: None,
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
