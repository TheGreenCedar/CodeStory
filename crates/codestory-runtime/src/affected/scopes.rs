use super::*;
use codestory_contracts::api::{
    AFFECTED_SCOPE_MAX_OWNERS, AffectedScopePublicationDto, AffectedScopeResolutionDto,
    AffectedScopeSideDto, AffectedScopeStatusDto as Status,
};
use codestory_indexer::source_units::{
    SourceUnitBudget, SourceUnitSelection, compile_source_units,
};

const MAX_SOURCE_FILES: usize = 32;
const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_WORK: usize = 8_000_000;

pub(super) struct Admission {
    pub seeds: BTreeMap<GraphNodeId, AffectedGraphEvidence>,
    pub resolutions: Vec<AffectedScopeResolutionDto>,
    pub seeded_records: Vec<bool>,
    pub seeded_files: BTreeSet<GraphNodeId>,
}

struct FileOwners {
    sha256: String,
    line_count: u32,
    declarations: Vec<GraphNode>,
    selections: Option<BTreeMap<u32, SourceUnitSelection>>,
}

fn declaration(kind: codestory_contracts::graph::NodeKind) -> bool {
    use codestory_contracts::graph::NodeKind::*;
    matches!(
        kind,
        STRUCT
            | CLASS
            | INTERFACE
            | ANNOTATION
            | UNION
            | ENUM
            | TYPEDEF
            | FUNCTION
            | METHOD
            | MACRO
            | GLOBAL_VARIABLE
            | FIELD
            | CONSTANT
            | ENUM_CONSTANT
    )
}

fn callable(kind: codestory_contracts::graph::NodeKind) -> bool {
    use codestory_contracts::graph::NodeKind::*;
    matches!(kind, FUNCTION | METHOD | MACRO)
}

fn spend(budget: &mut SourceUnitBudget, amount: usize) -> Result<(), Status> {
    if amount > budget.remaining_work {
        budget.remaining_work = 0;
        return Err(Status::BudgetExceeded);
    }
    budget.remaining_work -= amount;
    Ok(())
}

fn load_owners(
    storage: &Store,
    root: &Path,
    file: &FileInfo,
    graph: &AffectedGraphIndex,
    lines: &[usize],
    budget: &mut SourceUnitBudget,
) -> Result<Result<FileOwners, Status>, ApiError> {
    if budget.remaining_source_bytes == 0 {
        return Ok(Err(Status::BudgetExceeded));
    }
    let read_cap = budget.remaining_source_bytes.min(MAX_FILE_BYTES);
    let checked_path =
        crate::resolve_project_file_path_from_root(root, &file.path.to_string_lossy(), false)?;
    if std::fs::metadata(&checked_path)
        .map_err(|_| {
            ApiError::new(
                "source_unavailable",
                "Affected scope source metadata is unavailable.",
            )
        })?
        .len()
        > read_cap as u64
    {
        return Ok(Err(Status::BudgetExceeded));
    }
    let source =
        crate::search_evidence::verified_file_bounded(storage, root, file, read_cap as u64)
            .map_err(|error| match error {
                crate::search_evidence::VerifiedFileError::Stale => ApiError::new(
                    "source_stale",
                    "Affected scope source no longer matches the pinned publication.",
                ),
                crate::search_evidence::VerifiedFileError::Unavailable => ApiError::new(
                    "source_unavailable",
                    "Affected scope source could not be verified within its source bound.",
                ),
            })?;
    let mut declarations = Vec::new();
    for node_id in graph
        .node_ids_by_file
        .get(&codestory_contracts::graph::NodeId(file.id))
        .into_iter()
        .flatten()
    {
        if let Err(status) = spend(budget, 1) {
            return Ok(Err(status));
        }
        let Some(node) = graph.nodes_by_id.get(node_id) else {
            continue;
        };
        if declaration(node.kind) {
            declarations.push(node.clone());
        }
    }
    let extension = source
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let selections = if matches!(extension, "rs" | "go" | "py") {
        match compile_source_units(
            extension,
            &source.content,
            lines,
            budget,
            &crate::active_public_operation_cancellation()
                .map(codestory_indexer::CancellationToken::from_shared_flag)
                .unwrap_or_default(),
        ) {
            Ok(selections) => Some(
                selections
                    .into_iter()
                    .map(|selection| (selection.match_line as u32, selection))
                    .collect(),
            ),
            Err(
                codestory_indexer::source_units::SourceUnitError::WorkBudgetExceeded
                | codestory_indexer::source_units::SourceUnitError::SourceByteBudgetExceeded
                | codestory_indexer::source_units::SourceUnitError::DepthExceeded,
            ) => return Ok(Err(Status::BudgetExceeded)),
            Err(codestory_indexer::source_units::SourceUnitError::Cancelled) => {
                return Err(ApiError::new(
                    "cancelled",
                    "Affected scope admission was cancelled.",
                ));
            }
            Err(_) => return Ok(Err(Status::SourceUnavailable)),
        }
    } else {
        budget.remaining_source_bytes = budget
            .remaining_source_bytes
            .saturating_sub(source.content.len());
        None
    };
    Ok(Ok(FileOwners {
        sha256: source.content_sha256,
        line_count: source.content.lines().count().min(u32::MAX as usize) as u32,
        declarations,
        selections,
    }))
}

fn owner_at(
    file: &FileOwners,
    line: u32,
    budget: &mut SourceUnitBudget,
) -> Result<GraphNodeId, Status> {
    if line > file.line_count {
        return Err(Status::OutOfBounds);
    }
    spend(budget, file.declarations.len().saturating_mul(3))?;
    let candidates = file
        .declarations
        .iter()
        .filter(|node| {
            node.start_line
                .zip(node.end_line)
                .is_some_and(|(start, end)| start <= line && line <= end)
        })
        .collect::<Vec<_>>();
    // A parsed callable envelope can repair name-only graph display extents.
    if let Some(selection) = file
        .selections
        .as_ref()
        .and_then(|selections| selections.get(&line))
        && let Some(unit) = selection.units.iter().find(|unit| {
            unit.kind.contains("function")
                || unit.kind.contains("method")
                || unit.kind.ends_with("FunctionDef")
        })
    {
        let anchors = file
            .declarations
            .iter()
            .filter(|node| {
                callable(node.kind)
                    && node.start_line.is_some_and(|start| {
                        unit.start_line <= start as usize && start as usize <= unit.end_line
                    })
            })
            .collect::<Vec<_>>();
        let first = anchors.iter().filter_map(|node| node.start_line).min();
        let owners = anchors
            .into_iter()
            .filter(|node| node.start_line == first)
            .collect::<Vec<_>>();
        return match owners.as_slice() {
            [owner] => Ok(owner.id),
            [] => Err(Status::Unowned),
            _ => Err(Status::Ambiguous),
        };
    }
    let smallest = candidates
        .iter()
        .filter_map(|node| {
            node.start_line
                .zip(node.end_line)
                .map(|(start, end)| end - start)
        })
        .min();
    let owners = candidates
        .into_iter()
        .filter(|node| {
            node.start_line
                .zip(node.end_line)
                .map(|(start, end)| end - start)
                == smallest
        })
        .collect::<Vec<_>>();
    match owners.as_slice() {
        [owner] if file.selections.is_some() && callable(owner.kind) => Err(Status::Unowned),
        [owner] => Ok(owner.id),
        [] => Err(Status::Unowned),
        _ => Err(Status::Ambiguous),
    }
}

pub(super) fn admit(
    controller: &AppController,
    root: &Path,
    records: &[AffectedChangeRecordDto],
    files: &[FileInfo],
    graph: &AffectedGraphIndex,
    storage: &Store,
    matches: &AffectedIdentityMatches,
) -> Result<Admission, ApiError> {
    let mut indexed = HashMap::<WorkspacePathIdentity, Vec<GraphNodeId>>::new();
    for (file_id, identity) in &matches.indexed_identity_by_file_id {
        indexed.entry(identity.clone()).or_default().push(*file_id);
    }
    for ids in indexed.values_mut() {
        ids.sort_unstable();
    }
    let record_files = matches
        .current_identity_by_record
        .iter()
        .map(|identity| {
            identity
                .as_ref()
                .and_then(|identity| indexed.get(identity))
                .cloned()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let mut broad_files = BTreeSet::new();
    let mut broad_previous = HashMap::new();
    let mut seeded_records = vec![false; records.len()];
    let mut identities = AffectedOperationIdentityIndex::native();
    for (index, record) in records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.scope.is_none())
    {
        if !record_files[index].is_empty() {
            broad_files.extend(record_files[index].iter().copied());
            seeded_records[index] = true;
        } else if let Some(previous) = &record.previous_path
            && let Ok(identity) = identities.resolve(&affected_change_path(root, previous))
        {
            for file_id in indexed.get(&identity).into_iter().flatten() {
                broad_previous.entry(*file_id).or_insert(index);
                seeded_records[index] = true;
            }
        }
    }
    broad_previous.retain(|file_id, _| !broad_files.contains(file_id));
    let previous_evidence = previous_identity_seed_evidence(records, broad_previous);
    broad_files.extend(previous_evidence.keys().copied());
    let mut admission = Admission {
        seeds: affected_graph_seeds(&broad_files, &previous_evidence, &graph.node_ids_by_file),
        resolutions: Vec::new(),
        seeded_records,
        seeded_files: broad_files,
    };
    let publication = controller
        .active_core_publication()
        .or_else(|| storage.get_complete_index_publication().ok().flatten());
    let publication = publication
        .map(|publication| AffectedScopePublicationDto {
            project_id: codestory_workspace::project_identity_v3(root).project_id,
            generation_id: publication.generation_id,
            run_id: publication.run_id,
        })
        .ok_or_else(|| {
            ApiError::new(
                "source_unavailable",
                "Affected scopes require a pinned complete core publication.",
            )
        })?;
    let mut requested_lines = BTreeMap::<GraphNodeId, BTreeSet<usize>>::new();
    for (index, record) in records.iter().enumerate() {
        if let Some(current) = record
            .scope
            .as_ref()
            .and_then(|scope| scope.current.as_ref())
            && let [file_id] = record_files[index].as_slice()
        {
            for range in &current.ranges {
                requested_lines
                    .entry(*file_id)
                    .or_default()
                    .extend((range.start_line..=range.end_line).map(|line| line as usize));
            }
        }
    }
    let by_id = files
        .iter()
        .map(|file| (codestory_contracts::graph::NodeId(file.id), file))
        .collect::<HashMap<_, _>>();
    let mut cache = HashMap::<GraphNodeId, Result<FileOwners, Status>>::new();
    let mut budget = SourceUnitBudget {
        remaining_source_bytes: MAX_SOURCE_BYTES,
        remaining_work: MAX_WORK,
        max_depth: 256,
    };
    for (index, record) in records.iter().enumerate() {
        let Some(scope) = &record.scope else { continue };
        for (side, source) in [
            (AffectedScopeSideDto::Current, &scope.current),
            (AffectedScopeSideDto::Previous, &scope.previous),
        ] {
            let Some(source) = source else { continue };
            let file_id = match record_files[index].as_slice() {
                [id] => Some(*id),
                _ => None,
            };
            if side == AffectedScopeSideDto::Current
                && let Some(id) = file_id
                && !cache.contains_key(&id)
            {
                let owners = if cache.len() >= MAX_SOURCE_FILES {
                    Err(Status::BudgetExceeded)
                } else if let Some(file) = by_id.get(&id) {
                    let lines = requested_lines[&id].iter().copied().collect::<Vec<_>>();
                    load_owners(storage, root, file, graph, &lines, &mut budget)?
                } else {
                    Err(Status::SourceUnavailable)
                };
                cache.insert(id, owners);
            }
            for (range_index, range) in source.ranges.iter().enumerate() {
                if crate::active_public_operation_cancellation()
                    .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
                {
                    return Err(ApiError::new(
                        "cancelled",
                        "Affected scope admission was cancelled.",
                    ));
                }
                let mut row = AffectedScopeResolutionDto {
                    record_index: index as u32,
                    range_index: range_index as u32,
                    path: if side == AffectedScopeSideDto::Previous {
                        record
                            .previous_path
                            .as_ref()
                            .unwrap_or(&record.path)
                            .clone()
                    } else {
                        record.path.clone()
                    },
                    side,
                    range: range.clone(),
                    source_sha256: source.source_sha256.clone(),
                    status: Status::PreviousUnavailable,
                    owner_ids: Vec::new(),
                    publication: None,
                    verified_source_sha256: None,
                };
                if side == AffectedScopeSideDto::Current {
                    row.status = if record_files[index].len() > 1 {
                        Status::Ambiguous
                    } else {
                        Status::SourceUnavailable
                    };
                    if let Some(id) = file_id {
                        match &cache[&id] {
                            Err(status) => row.status = *status,
                            Ok(file) => {
                                row.publication = Some(publication.clone());
                                row.verified_source_sha256 = Some(file.sha256.clone());
                                if source.source_sha256 != file.sha256 {
                                    row.status = Status::CallerHashMismatch;
                                } else {
                                    let mut owners = BTreeSet::new();
                                    let mut gaps = Vec::new();
                                    for line in range.start_line..=range.end_line {
                                        match owner_at(file, line, &mut budget) {
                                            Ok(owner) => {
                                                owners.insert(owner);
                                            }
                                            Err(status) => gaps.push(status),
                                        }
                                        if owners.len() > AFFECTED_SCOPE_MAX_OWNERS
                                            || gaps.contains(&Status::BudgetExceeded)
                                        {
                                            break;
                                        }
                                    }
                                    row.status = if owners.len() > AFFECTED_SCOPE_MAX_OWNERS
                                        || gaps.contains(&Status::BudgetExceeded)
                                    {
                                        Status::BudgetExceeded
                                    } else if owners.is_empty() {
                                        gaps.first().copied().unwrap_or(Status::Unowned)
                                    } else if !gaps.is_empty() {
                                        Status::Partial
                                    } else if owners.len() > 1 {
                                        Status::CrossOwner
                                    } else {
                                        Status::Resolved
                                    };
                                    // Only independently resolved portions become seeds; ties never do.
                                    for owner in owners.into_iter().take(AFFECTED_SCOPE_MAX_OWNERS)
                                    {
                                        admission.seeds.entry(owner).or_insert_with(|| {
                                            AffectedGraphEvidence::seed(
                                                owner,
                                                "declaration owns verified changed source range",
                                                AffectedConfidenceFloor::from_label("direct"),
                                                false,
                                            )
                                        });
                                        admission.seeded_records[index] = true;
                                        admission.seeded_files.insert(id);
                                        row.owner_ids.push(NodeId::from(owner));
                                    }
                                }
                            }
                        }
                    }
                }
                admission.resolutions.push(row);
            }
        }
    }
    Ok(admission)
}
