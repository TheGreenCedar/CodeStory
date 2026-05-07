use super::*;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Debug, Clone, Copy)]
struct GroundingBudgetConfig {
    root_symbols: usize,
    symbols_per_file: usize,
    expanded_files: usize,
    coverage_buckets: usize,
    sample_paths_per_bucket: usize,
}

fn budget_config(budget: GroundingBudgetDto) -> GroundingBudgetConfig {
    match budget {
        GroundingBudgetDto::Strict => GroundingBudgetConfig {
            root_symbols: 8,
            symbols_per_file: 2,
            expanded_files: 8,
            coverage_buckets: 4,
            sample_paths_per_bucket: 2,
        },
        GroundingBudgetDto::Balanced => GroundingBudgetConfig {
            root_symbols: 16,
            symbols_per_file: 4,
            expanded_files: 16,
            coverage_buckets: 6,
            sample_paths_per_bucket: 3,
        },
        GroundingBudgetDto::Max => GroundingBudgetConfig {
            root_symbols: 28,
            symbols_per_file: 8,
            expanded_files: 32,
            coverage_buckets: 8,
            sample_paths_per_bucket: 4,
        },
    }
}

fn is_import_like_symbol(node: &codestory_contracts::graph::Node) -> bool {
    matches!(
        node.kind,
        codestory_contracts::graph::NodeKind::MODULE
            | codestory_contracts::graph::NodeKind::NAMESPACE
            | codestory_contracts::graph::NodeKind::PACKAGE
    ) && is_import_like_name(&node_display_name(node))
}

fn is_import_like_name(name: &str) -> bool {
    let trimmed = name.trim();
    (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        || (trimmed.starts_with('<') && trimmed.ends_with('>'))
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.contains('/')
}

fn node_rank(node: &codestory_contracts::graph::Node) -> u8 {
    if is_import_like_symbol(node) {
        return 5;
    }

    match node.kind {
        codestory_contracts::graph::NodeKind::CLASS
        | codestory_contracts::graph::NodeKind::STRUCT
        | codestory_contracts::graph::NodeKind::INTERFACE
        | codestory_contracts::graph::NodeKind::ENUM
        | codestory_contracts::graph::NodeKind::UNION
        | codestory_contracts::graph::NodeKind::ANNOTATION
        | codestory_contracts::graph::NodeKind::TYPEDEF => 0,
        codestory_contracts::graph::NodeKind::FUNCTION
        | codestory_contracts::graph::NodeKind::METHOD
        | codestory_contracts::graph::NodeKind::MACRO => 1,
        codestory_contracts::graph::NodeKind::MODULE
        | codestory_contracts::graph::NodeKind::NAMESPACE
        | codestory_contracts::graph::NodeKind::PACKAGE => 2,
        codestory_contracts::graph::NodeKind::FIELD
        | codestory_contracts::graph::NodeKind::VARIABLE
        | codestory_contracts::graph::NodeKind::GLOBAL_VARIABLE
        | codestory_contracts::graph::NodeKind::CONSTANT
        | codestory_contracts::graph::NodeKind::ENUM_CONSTANT
        | codestory_contracts::graph::NodeKind::TYPE_PARAMETER => 3,
        _ => 4,
    }
}

fn compare_nodes(
    left: &codestory_contracts::graph::Node,
    right: &codestory_contracts::graph::Node,
) -> Ordering {
    node_rank(left)
        .cmp(&node_rank(right))
        .then(
            left.start_line
                .unwrap_or(u32::MAX)
                .cmp(&right.start_line.unwrap_or(u32::MAX)),
        )
        .then_with(|| node_display_name(left).cmp(&node_display_name(right)))
        .then(left.id.0.cmp(&right.id.0))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn bucket_label_for_path(path: &str) -> String {
    let mut segments = path.split('/');
    let first = segments.next().unwrap_or("(root)");
    if segments.next().is_some() {
        first.to_string()
    } else {
        "(root)".to_string()
    }
}

#[derive(Debug)]
struct FileCoverage {
    file: FileInfo,
    relative_path: String,
    total_symbol_count: u32,
    represented_symbol_count: u32,
    best_node_rank: u8,
}

fn compare_file_coverage(left: &FileCoverage, right: &FileCoverage) -> Ordering {
    left.best_node_rank
        .cmp(&right.best_node_rank)
        .then(right.total_symbol_count.cmp(&left.total_symbol_count))
        .then_with(|| left.relative_path.cmp(&right.relative_path))
}

fn build_coverage_buckets(
    omitted: &[FileCoverage],
    max_buckets: usize,
    sample_paths_per_bucket: usize,
) -> Vec<GroundingCoverageBucketDto> {
    if omitted.is_empty() || max_buckets == 0 {
        return Vec::new();
    }

    let mut grouped = BTreeMap::<String, Vec<&FileCoverage>>::new();
    for file in omitted {
        grouped
            .entry(bucket_label_for_path(&file.relative_path))
            .or_default()
            .push(file);
    }

    let mut buckets = grouped
        .into_iter()
        .map(|(label, entries)| {
            let mut sample_paths = entries
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect::<Vec<_>>();
            sample_paths.sort();
            sample_paths.truncate(sample_paths_per_bucket);

            GroundingCoverageBucketDto {
                label,
                file_count: entries.len().min(u32::MAX as usize) as u32,
                symbol_count: entries.iter().map(|entry| entry.total_symbol_count).sum(),
                sample_paths,
            }
        })
        .collect::<Vec<_>>();
    buckets.sort_by(|left, right| {
        right
            .file_count
            .cmp(&left.file_count)
            .then(right.symbol_count.cmp(&left.symbol_count))
            .then_with(|| left.label.cmp(&right.label))
    });

    if buckets.len() <= max_buckets {
        return buckets;
    }

    let keep = max_buckets.saturating_sub(1);
    let mut overflow = buckets.split_off(keep);
    let mut sample_paths = overflow
        .iter_mut()
        .flat_map(|bucket| std::mem::take(&mut bucket.sample_paths))
        .collect::<Vec<_>>();
    sample_paths.sort();
    sample_paths.dedup();
    sample_paths.truncate(sample_paths_per_bucket);

    let other = GroundingCoverageBucketDto {
        label: "other".to_string(),
        file_count: overflow.iter().map(|bucket| bucket.file_count).sum(),
        symbol_count: overflow.iter().map(|bucket| bucket.symbol_count).sum(),
        sample_paths,
    };
    buckets.push(other);
    buckets
}

fn symbol_digest(
    node: &codestory_contracts::graph::Node,
    display_name: &str,
    relative_file_path: Option<&str>,
    member_counts: &HashMap<codestory_contracts::graph::NodeId, u32>,
    fallback_lines: &HashMap<codestory_contracts::graph::NodeId, u32>,
    edge_digests: &HashMap<codestory_contracts::graph::NodeId, Vec<String>>,
    summaries: &HashMap<codestory_contracts::graph::NodeId, SymbolSummaryRecord>,
) -> GroundingSymbolDigestDto {
    let member_count = if is_structural_kind(node.kind) {
        Some(*member_counts.get(&node.id).unwrap_or(&0))
    } else {
        None
    };

    let line = node
        .start_line
        .or_else(|| fallback_lines.get(&node.id).copied());

    let label = if let Some(file_path) = relative_file_path {
        format!("{display_name} @ {file_path}")
    } else {
        display_name.to_string()
    };

    GroundingSymbolDigestDto {
        id: NodeId::from(node.id),
        node_ref: relative_file_path
            .zip(line)
            .map(|(path, line)| format!("{path}:{line}:{display_name}")),
        label,
        kind: NodeKind::from(node.kind),
        line,
        member_count,
        summary: summaries.get(&node.id).map(|record| record.summary.clone()),
        edge_digest: edge_digests.get(&node.id).cloned().unwrap_or_default(),
    }
}

fn dedupe_grounding_node_records(nodes: Vec<GroundingNodeRecord>) -> Vec<GroundingNodeRecord> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(nodes.len());
    for record in nodes {
        let key = (
            record.node.kind as i32,
            record.display_name.clone(),
            record.node.file_node_id,
        );
        if seen.insert(key) {
            deduped.push(record);
        }
    }
    deduped
}

fn build_edge_digest_map(
    counts: Vec<GroundingEdgeKindCount>,
    limit: usize,
) -> HashMap<codestory_contracts::graph::NodeId, Vec<String>> {
    let mut grouped = HashMap::<codestory_contracts::graph::NodeId, Vec<(String, u32)>>::new();
    for entry in counts {
        grouped
            .entry(entry.node_id)
            .or_default()
            .push((format!("{:?}", entry.kind), entry.count));
    }

    grouped
        .into_iter()
        .map(|(node_id, mut digests)| {
            digests.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
            let items = digests
                .into_iter()
                .take(limit)
                .map(|(kind, count)| format!("{kind}={count}"))
                .collect::<Vec<_>>();
            (node_id, items)
        })
        .collect()
}

impl AppController {
    pub fn grounding_snapshot(
        &self,
        budget: GroundingBudgetDto,
    ) -> Result<GroundingSnapshotDto, ApiError> {
        self.ensure_consistent_read_state("Grounding")?;
        let root = self.require_project_root()?;
        let storage = self.open_storage()?;
        if matches!(budget, GroundingBudgetDto::Max)
            && !storage.snapshots().has_ready_detail().map_err(|e| {
                ApiError::internal(format!(
                    "Failed to query grounding detail snapshot readiness: {e}"
                ))
            })?
        {
            let _guard = self.grounding_detail_refresh.lock();
            if !storage.snapshots().has_ready_detail().map_err(|e| {
                ApiError::internal(format!(
                    "Failed to query grounding detail snapshot readiness: {e}"
                ))
            })? {
                storage.snapshots().refresh_detail().map_err(|e| {
                    ApiError::internal(format!("Failed to hydrate grounding detail snapshots: {e}"))
                })?;
            }
        }
        let config = budget_config(budget);

        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;
        let file_summaries = storage.get_grounding_file_summaries().map_err(|e| {
            ApiError::internal(format!("Failed to load grounding file summaries: {e}"))
        })?;
        let derived_file_count = if stats.file_count > 0 {
            stats.file_count
        } else {
            file_summaries.len().min(i64::MAX as usize) as i64
        };
        let dto_stats = StorageStatsDto {
            node_count: clamp_i64_to_u32(stats.node_count),
            edge_count: clamp_i64_to_u32(stats.edge_count),
            file_count: clamp_i64_to_u32(derived_file_count),
            error_count: clamp_i64_to_u32(stats.error_count),
        };

        let mut file_coverages = Vec::with_capacity(file_summaries.len());
        for summary in file_summaries {
            file_coverages.push(FileCoverage {
                relative_path: relative_path(&root, &summary.file.path),
                total_symbol_count: summary.symbol_count,
                represented_symbol_count: summary.symbol_count.min(config.symbols_per_file as u32),
                best_node_rank: summary.best_node_rank,
                file: summary.file,
            });
        }
        file_coverages.sort_by(compare_file_coverage);

        let expanded_files = file_coverages.len().min(config.expanded_files);
        let omitted_files = file_coverages.len().saturating_sub(expanded_files);
        let expanded_file_ids = file_coverages
            .iter()
            .take(expanded_files)
            .map(|coverage| coverage.file.id)
            .collect::<Vec<_>>();
        let mut file_nodes_by_id = BTreeMap::<i64, Vec<GroundingNodeRecord>>::new();
        for record in storage
            .get_grounding_top_symbols_for_files(&expanded_file_ids, config.symbols_per_file)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load grounding file symbols: {e}"))
            })?
        {
            if let Some(file_node_id) = record.node.file_node_id {
                file_nodes_by_id
                    .entry(file_node_id.0)
                    .or_default()
                    .push(record);
            }
        }

        let mut compressed_files = omitted_files.min(u32::MAX as usize) as u32;
        let mut file_digests = Vec::with_capacity(expanded_files);
        let mut omitted_coverages = Vec::with_capacity(omitted_files);
        let mut selected_coverages = Vec::with_capacity(expanded_files);
        let mut displayed_file_nodes = Vec::<GroundingNodeRecord>::new();
        for (index, coverage) in file_coverages.into_iter().enumerate() {
            if index >= expanded_files {
                omitted_coverages.push(coverage);
                continue;
            }
            displayed_file_nodes.extend(
                file_nodes_by_id
                    .get(&coverage.file.id)
                    .into_iter()
                    .flat_map(|records| records.iter().cloned()),
            );
            selected_coverages.push(coverage);
        }
        let coverage_buckets = build_coverage_buckets(
            &omitted_coverages,
            config.coverage_buckets,
            config.sample_paths_per_bucket,
        );
        let bucketed_files = coverage_buckets
            .iter()
            .map(|bucket| bucket.file_count)
            .sum::<u32>();
        let bucketed_symbols = coverage_buckets
            .iter()
            .map(|bucket| bucket.symbol_count)
            .sum::<u32>();
        let root_fetch_limit = config
            .root_symbols
            .saturating_mul(8)
            .max(config.root_symbols);
        let mut root_records = Vec::new();
        let mut root_offset = 0usize;
        loop {
            let page = storage
                .get_grounding_root_symbol_candidates(root_fetch_limit, root_offset)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to load grounding root symbols: {e}"))
                })?;
            if page.is_empty() {
                break;
            }
            let fetched = page.len();
            root_offset = root_offset.saturating_add(page.len());
            root_records.extend(page);
            root_records = dedupe_grounding_node_records(root_records);
            if root_records.len() >= config.root_symbols || fetched < root_fetch_limit {
                break;
            }
        }
        root_records.truncate(config.root_symbols);

        let mut structural_ids = displayed_file_nodes
            .iter()
            .chain(root_records.iter())
            .filter(|record| is_structural_kind(record.node.kind))
            .map(|record| record.node.id)
            .collect::<Vec<_>>();
        structural_ids.sort_by_key(|id| id.0);
        structural_ids.dedup();
        let member_counts = storage
            .get_grounding_member_counts(&structural_ids)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load grounding member counts: {e}"))
            })?;
        let mut missing_line_ids = displayed_file_nodes
            .iter()
            .chain(root_records.iter())
            .filter(|record| record.node.start_line.is_none())
            .map(|record| record.node.id)
            .collect::<Vec<_>>();
        missing_line_ids.sort_by_key(|id| id.0);
        missing_line_ids.dedup();
        let fallback_lines = storage
            .get_grounding_min_occurrence_lines(&missing_line_ids)
            .map_err(|e| {
                ApiError::internal(format!("Failed to load grounding line fallbacks: {e}"))
            })?;
        let mut displayed_node_ids = displayed_file_nodes
            .iter()
            .chain(root_records.iter())
            .map(|record| record.node.id)
            .collect::<Vec<_>>();
        displayed_node_ids.sort_by_key(|id| id.0);
        displayed_node_ids.dedup();
        let edge_digests = build_edge_digest_map(
            storage
                .get_grounding_edge_digest_counts(&displayed_node_ids)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to load grounding edge digests: {e}"))
                })?,
            4,
        );
        let summaries = storage
            .get_current_symbol_summaries_by_node_ids(&displayed_node_ids)
            .map_err(|e| ApiError::internal(format!("Failed to load symbol summaries: {e}")))?;

        for coverage in selected_coverages {
            let mut symbols = Vec::with_capacity(coverage.represented_symbol_count as usize);
            if let Some(records) = file_nodes_by_id.get(&coverage.file.id) {
                for record in records {
                    let relative_file_path = record
                        .file_path
                        .as_deref()
                        .map(|path| relative_path(&root, path));
                    symbols.push(symbol_digest(
                        &record.node,
                        &record.display_name,
                        relative_file_path.as_deref(),
                        &member_counts,
                        &fallback_lines,
                        &edge_digests,
                        &summaries,
                    ));
                }
            }
            if coverage.total_symbol_count > coverage.represented_symbol_count {
                compressed_files = compressed_files.saturating_add(1);
            }

            file_digests.push(GroundingFileDigestDto {
                file_path: coverage.relative_path,
                language: (!coverage.file.language.trim().is_empty())
                    .then_some(coverage.file.language),
                symbol_count: coverage.total_symbol_count,
                represented_symbol_count: coverage.represented_symbol_count,
                compressed: coverage.total_symbol_count > coverage.represented_symbol_count,
                symbols,
            });
        }

        let represented_symbols = file_digests
            .iter()
            .map(|file| file.symbol_count)
            .sum::<u32>()
            .saturating_add(bucketed_symbols);

        let mut root_symbols = Vec::new();
        for record in &root_records {
            let relative_file_path = record
                .file_path
                .as_deref()
                .map(|path| relative_path(&root, path));
            root_symbols.push(symbol_digest(
                &record.node,
                &record.display_name,
                relative_file_path.as_deref(),
                &member_counts,
                &fallback_lines,
                &edge_digests,
                &summaries,
            ));
        }

        let mut recommended_queries = Vec::new();
        for node in root_symbols.iter().take(5) {
            let trimmed = node
                .label
                .split('@')
                .next()
                .map(str::trim)
                .unwrap_or(node.label.as_str());
            if !trimmed.is_empty() {
                recommended_queries.push(trimmed.to_string());
            }
        }

        let mut notes = vec![
            "Use `search --query <term>` to locate a symbol quickly.".to_string(),
            "Use `symbol --query <term>` for members, related hits, and edge digest.".to_string(),
            "Use `trail --query <term>` for neighborhood or call-path context.".to_string(),
            "Use `snippet --query <term>` for source text around a symbol.".to_string(),
        ];
        if compressed_files > 0 {
            notes.push(format!(
                "{compressed_files} file(s) were compressed to stay within the {budget:?} grounding budget."
            ));
        }
        if omitted_files > 0 {
            notes.push(format!(
                "{} file(s) are shown in detail; {} more are summarized into {} coverage bucket(s).",
                file_digests.len(),
                omitted_files,
                coverage_buckets.len()
            ));
        }

        let total_file_count = dto_stats.file_count;
        let retrieval = retrieval_state_from_storage(&storage).ok();
        if let Some(state) = retrieval.as_ref() {
            let mode = match state.mode {
                codestory_contracts::api::RetrievalModeDto::Hybrid => "hybrid",
                codestory_contracts::api::RetrievalModeDto::Symbolic => "symbolic",
            };
            let mut retrieval_note = format!(
                "Retrieval mode: {mode} (semantic_docs={}).",
                state.semantic_doc_count
            );
            if let Some(reason) = state.fallback_reason {
                let reason = match reason {
                    codestory_contracts::api::RetrievalFallbackReasonDto::DisabledByConfig => {
                        "disabled_by_config"
                    }
                    codestory_contracts::api::RetrievalFallbackReasonDto::MissingEmbeddingRuntime => {
                        "missing_embedding_runtime"
                    }
                    codestory_contracts::api::RetrievalFallbackReasonDto::MissingSemanticDocs => {
                        "missing_semantic_docs"
                    }
                };
                retrieval_note.push_str(&format!(" fallback={reason}."));
            }
            notes.push(retrieval_note);
        }

        Ok(GroundingSnapshotDto {
            root: root.to_string_lossy().to_string(),
            budget,
            generated_at_epoch_ms: current_epoch_ms(),
            stats: dto_stats,
            retrieval,
            coverage: codestory_contracts::api::GroundingCoverageDto {
                total_files: total_file_count,
                represented_files: (file_digests.len().min(u32::MAX as usize) as u32)
                    .saturating_add(bucketed_files)
                    .min(total_file_count),
                total_symbols: file_digests
                    .iter()
                    .map(|file| file.symbol_count)
                    .sum::<u32>()
                    .saturating_add(bucketed_symbols),
                represented_symbols,
                compressed_files,
            },
            root_symbols,
            files: file_digests,
            coverage_buckets,
            notes,
            recommended_queries,
        })
    }

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        let storage = self.open_storage()?;
        let node = self.node_details(NodeDetailsRequest {
            id: node_id.clone(),
        })?;
        let core_id = node_id.to_core()?;

        let mut children = storage
            .get_children_symbols(core_id)
            .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?;
        children.sort_by(compare_nodes);
        let labels_by_id = self.cached_labels(children.iter().map(|child| child.id));
        let children = Self::dedupe_symbol_nodes(children, &labels_by_id)
            .into_iter()
            .take(16)
            .map(|child| Self::symbol_summary_for_node(&storage, &labels_by_id, child))
            .collect::<Result<Vec<_>, ApiError>>()?;

        let related_hits = self
            .lexical_symbol_hits(&node.display_name, 6)?
            .into_iter()
            .filter(|hit| hit.node_id != node_id)
            .take(6)
            .collect();
        let summary = storage
            .get_current_symbol_summaries_by_node_ids(&[core_id])
            .map_err(|e| ApiError::internal(format!("Failed to load symbol summary: {e}")))?
            .remove(&core_id)
            .map(|record| record.summary);

        Ok(SymbolContextDto {
            node,
            summary,
            children,
            related_hits,
            edge_digest: edge_digest_for_node(&storage, core_id, 6),
        })
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        let focus = self.node_details(NodeDetailsRequest {
            id: req.root_id.clone(),
        })?;
        let story_requested = req.story;
        let trail = self.graph_trail(req.clone())?;
        let story = if story_requested {
            let project_root = self.require_project_root().ok();
            Some(build_trail_story(
                project_root.as_deref(),
                &focus,
                &trail,
                &req,
            ))
        } else {
            None
        };
        Ok(TrailContextDto {
            focus,
            trail,
            story,
        })
    }

    pub fn snippet_context(
        &self,
        node_id: NodeId,
        context_lines: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        let node = self.node_details(NodeDetailsRequest { id: node_id })?;
        let path = node
            .file_path
            .clone()
            .ok_or_else(|| ApiError::invalid_argument("Symbol has no source file."))?;
        let line = node
            .start_line
            .ok_or_else(|| ApiError::invalid_argument("Symbol has no source line."))?;
        let (path, bounded) = self.bounded_file_snippet(
            &path,
            line,
            context_lines,
            crate::DIRECT_SNIPPET_MAX_BYTES,
            crate::DIRECT_SNIPPET_TRUNCATION_SUFFIX,
        )?;

        Ok(SnippetContextDto {
            node,
            path,
            line,
            snippet: bounded.markdown,
            snippet_truncated: bounded.truncated,
            max_snippet_bytes: Some(crate::DIRECT_SNIPPET_MAX_BYTES as u32),
        })
    }
}

const TRAIL_STORY_CORE_FLOW_LIMIT: usize = 16;
const TRAIL_STORY_PREVIEW_LIMIT: usize = 5;

fn build_trail_story(
    project_root: Option<&Path>,
    focus: &NodeDetailsDto,
    trail: &GraphResponse,
    req: &TrailConfigDto,
) -> TrailStoryDto {
    let nodes_by_id = trail
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut incoming_counts = HashMap::<_, u32>::new();
    for edge in &trail.edges {
        *incoming_counts.entry(edge.target.clone()).or_default() += 1;
    }

    let test_nodes = trail
        .nodes
        .iter()
        .filter(|node| is_test_like_story_node(node))
        .collect::<Vec<_>>();

    let mut entry_points = Vec::new();
    entry_points.push(format!("focus: {}", story_focus_ref(project_root, focus)));
    for node in trail
        .nodes
        .iter()
        .filter(|node| incoming_counts.get(&node.id).copied().unwrap_or_default() == 0)
        .filter(|node| node.id != focus.id)
        .take(TRAIL_STORY_PREVIEW_LIMIT)
    {
        entry_points.push(format!("entry: {}", story_node_ref(project_root, node)));
    }
    if entry_points.len() == 1 && trail.edges.is_empty() {
        entry_points.push("no graph entry edges were returned for this focus".to_string());
    }

    let core_flow = trail
        .edges
        .iter()
        .take(TRAIL_STORY_CORE_FLOW_LIMIT)
        .map(|edge| story_step(project_root, edge, &nodes_by_id))
        .collect::<Vec<_>>();
    let side_effects = side_effects_for_story(project_root, trail, &nodes_by_id);
    let uncertainty = uncertainty_for_story(project_root, trail, &nodes_by_id, req);
    let test_scope = test_scope_for_story(project_root, req, &test_nodes);
    let limits = limits_for_story(trail, req);
    let summary = format!(
        "Story trail around `{}` found {} nodes and {} edges; mode={} direction={} tests={} utility_calls={} truncated={}.",
        focus.display_name,
        trail.nodes.len(),
        trail.edges.len(),
        story_trail_mode(req.mode),
        story_trail_direction(req.direction),
        if req.caller_scope == TrailCallerScope::IncludeTestsAndBenches {
            "included"
        } else {
            "excluded"
        },
        if req.show_utility_calls {
            "included"
        } else {
            "hidden"
        },
        trail.truncated
    );

    TrailStoryDto {
        summary,
        entry_points,
        core_flow,
        side_effects,
        uncertainty,
        test_scope,
        limits,
    }
}

fn story_step(
    project_root: Option<&Path>,
    edge: &GraphEdgeDto,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> TrailStoryStepDto {
    let source = nodes_by_id
        .get(&edge.source)
        .map(|node| story_node_ref(project_root, node))
        .unwrap_or_else(|| edge.source.0.clone());
    let target = nodes_by_id
        .get(&edge.target)
        .map(|node| story_node_ref(project_root, node))
        .unwrap_or_else(|| edge.target.0.clone());
    let relation = story_relation(edge.kind).to_string();
    let certainty = story_certainty(edge);
    let confidence = edge
        .confidence
        .map(|value| format!(" confidence={value:.2}"))
        .unwrap_or_default();
    let candidates = if edge.candidate_targets.is_empty() {
        String::new()
    } else {
        format!(" candidate_targets={}", edge.candidate_targets.len())
    };
    let callsite = edge
        .callsite_identity
        .as_deref()
        .map(|value| format!(" callsite={value}"))
        .unwrap_or_default();
    let note = format!(
        "{} {} edge{}{}{}",
        certainty,
        format!("{:?}", edge.kind).to_lowercase(),
        confidence,
        candidates,
        callsite
    );

    TrailStoryStepDto {
        edge_id: edge.id.0.clone(),
        source,
        relation,
        target,
        certainty,
        note,
    }
}

fn story_node_ref(project_root: Option<&Path>, node: &GraphNodeDto) -> String {
    let path = node
        .file_path
        .as_deref()
        .map(|value| format!(" `{}`", story_path(project_root, value)))
        .unwrap_or_default();
    format!("{} [{}]{}", node.label, story_node_kind(node.kind), path)
}

fn story_focus_ref(project_root: Option<&Path>, node: &NodeDetailsDto) -> String {
    let path = node
        .file_path
        .as_deref()
        .map(|value| format!(" `{}`", story_path(project_root, value)))
        .unwrap_or_default();
    format!(
        "{} [{}]{}",
        node.display_name,
        story_node_kind(node.kind),
        path
    )
}

fn story_path(project_root: Option<&Path>, value: &str) -> String {
    let path = Path::new(value);
    project_root
        .and_then(|root| path.strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| value.replace('\\', "/"))
}

fn story_node_kind(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::MODULE => "module",
        NodeKind::NAMESPACE => "namespace",
        NodeKind::PACKAGE => "package",
        NodeKind::FILE => "file",
        NodeKind::STRUCT => "struct",
        NodeKind::CLASS => "class",
        NodeKind::INTERFACE => "interface",
        NodeKind::ANNOTATION => "annotation",
        NodeKind::UNION => "union",
        NodeKind::ENUM => "enum",
        NodeKind::TYPEDEF => "typedef",
        NodeKind::TYPE_PARAMETER => "type_parameter",
        NodeKind::BUILTIN_TYPE => "builtin_type",
        NodeKind::FUNCTION => "function",
        NodeKind::METHOD => "method",
        NodeKind::MACRO => "macro",
        NodeKind::GLOBAL_VARIABLE => "global_variable",
        NodeKind::FIELD => "field",
        NodeKind::VARIABLE => "variable",
        NodeKind::CONSTANT => "constant",
        NodeKind::ENUM_CONSTANT => "enum_constant",
        NodeKind::UNKNOWN => "unknown",
    }
}

fn story_relation(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::CALL => "calls",
        EdgeKind::USAGE => "uses",
        EdgeKind::TYPE_USAGE => "uses type",
        EdgeKind::MEMBER => "contains",
        EdgeKind::INHERITANCE => "inherits from",
        EdgeKind::OVERRIDE => "overrides",
        EdgeKind::TYPE_ARGUMENT => "passes type argument to",
        EdgeKind::TEMPLATE_SPECIALIZATION => "specializes",
        EdgeKind::INCLUDE => "includes",
        EdgeKind::IMPORT => "imports",
        EdgeKind::MACRO_USAGE => "uses macro",
        EdgeKind::ANNOTATION_USAGE => "uses annotation",
        EdgeKind::UNKNOWN => "relates to",
    }
}

fn story_certainty(edge: &GraphEdgeDto) -> String {
    edge.certainty
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "missing certainty metadata".to_string())
}

fn is_uncertain_story_certainty(certainty: &str) -> bool {
    matches!(
        certainty,
        "probable" | "uncertain" | "speculative" | "missing certainty metadata"
    )
}

fn side_effects_for_story(
    project_root: Option<&Path>,
    trail: &GraphResponse,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
) -> Vec<String> {
    let mut side_effects = Vec::new();
    for edge in &trail.edges {
        let target = nodes_by_id.get(&edge.target).copied();
        if !edge_suggests_side_effect(edge, target) {
            continue;
        }
        let step = story_step(project_root, edge, nodes_by_id);
        side_effects.push(format!(
            "possible side-effect candidate [{}] {} {} {} (certainty={})",
            step.edge_id, step.source, step.relation, step.target, step.certainty
        ));
    }
    if side_effects.is_empty() {
        side_effects.push(
            "none detected from conservative edge-kind and target-name heuristics; inspect snippets for runtime effects"
                .to_string(),
        );
    }
    side_effects
}

fn edge_suggests_side_effect(edge: &GraphEdgeDto, target: Option<&GraphNodeDto>) -> bool {
    if edge.kind != EdgeKind::CALL {
        return false;
    }
    let Some(target) = target else {
        return false;
    };
    let tokens = story_identifier_tokens(&target.label);
    tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "write"
                | "save"
                | "persist"
                | "update"
                | "insert"
                | "delete"
                | "remove"
                | "emit"
                | "send"
                | "flush"
                | "commit"
                | "publish"
        )
    })
}

fn story_identifier_tokens(value: &str) -> Vec<String> {
    let mut normalized = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                normalized.push(' ');
            }
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn uncertainty_for_story(
    project_root: Option<&Path>,
    trail: &GraphResponse,
    nodes_by_id: &HashMap<NodeId, &GraphNodeDto>,
    req: &TrailConfigDto,
) -> Vec<String> {
    let mut uncertainty = Vec::new();
    if req.hide_speculative {
        uncertainty.push(
            "hide_speculative was applied before story rendering; uncertain/speculative edges may have been removed"
                .to_string(),
        );
    }
    if !req.edge_filter.is_empty() {
        uncertainty.push("edge filters were applied before rendering".to_string());
    }
    for edge in &trail.edges {
        let certainty = story_certainty(edge);
        if !is_uncertain_story_certainty(&certainty) {
            continue;
        }
        let step = story_step(project_root, edge, nodes_by_id);
        uncertainty.push(format!(
            "[{}] {} {} {} is {}. {}",
            step.edge_id, step.source, step.relation, step.target, step.certainty, step.note
        ));
    }
    if uncertainty.is_empty() {
        if trail.edges.is_empty() {
            uncertainty.push("no rendered trail edges to evaluate for certainty".to_string());
        } else {
            uncertainty.push("all rendered trail edges are explicitly marked certain".to_string());
        }
    }
    uncertainty
}

fn test_scope_for_story(
    project_root: Option<&Path>,
    req: &TrailConfigDto,
    test_nodes: &[&GraphNodeDto],
) -> Vec<String> {
    let mut scope = Vec::new();
    if req.caller_scope == TrailCallerScope::IncludeTestsAndBenches {
        scope.push("tests and benches included by request caller scope".to_string());
    } else {
        scope.push(
            "tests and benches excluded by production-only caller scope; request IncludeTestsAndBenches to include them"
                .to_string(),
        );
    }
    if test_nodes.is_empty() {
        scope.push("no test-like nodes are present in the rendered trail".to_string());
    } else {
        scope.push(format!(
            "{} test-like node(s) present: {}",
            test_nodes.len(),
            test_nodes
                .iter()
                .take(TRAIL_STORY_PREVIEW_LIMIT)
                .map(|node| story_node_ref(project_root, node))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    scope.push(if req.show_utility_calls {
        "utility/helper calls included by request".to_string()
    } else {
        "utility/helper calls hidden by default; enable show_utility_calls to include them"
            .to_string()
    });
    scope
}

fn limits_for_story(trail: &GraphResponse, req: &TrailConfigDto) -> Vec<String> {
    let mut limits = Vec::new();
    if trail.edges.len() > TRAIL_STORY_CORE_FLOW_LIMIT {
        limits.push(format!(
            "core_flow shows first {} of {} rendered edges",
            TRAIL_STORY_CORE_FLOW_LIMIT,
            trail.edges.len()
        ));
    }
    if trail.truncated {
        limits.push(format!(
            "trail was truncated at max_nodes={} with omitted_edge_count={}",
            req.max_nodes, trail.omitted_edge_count
        ));
    } else {
        limits.push(format!(
            "trail not truncated; max_nodes={} omitted_edge_count={}",
            req.max_nodes, trail.omitted_edge_count
        ));
    }
    if trail.edges.is_empty() {
        limits
            .push("no edges were returned, so core flow is limited to the focus node".to_string());
    }
    limits
}

fn is_test_like_story_node(node: &GraphNodeDto) -> bool {
    let text = format!(
        "{} {}",
        node.label.to_ascii_lowercase(),
        node.file_path
            .as_deref()
            .unwrap_or_default()
            .replace('\\', "/")
            .to_ascii_lowercase()
    );
    text.contains("/test")
        || text.contains("tests/")
        || text.contains("_test")
        || text.contains("test_")
        || text.contains("/benches/")
        || text.contains("bench_")
}

fn story_trail_mode(mode: TrailMode) -> &'static str {
    match mode {
        TrailMode::Neighborhood => "neighborhood",
        TrailMode::AllReferenced => "referenced",
        TrailMode::AllReferencing => "referencing",
        TrailMode::ToTargetSymbol => "to_target_symbol",
    }
}

fn story_trail_direction(direction: TrailDirection) -> &'static str {
    match direction {
        TrailDirection::Incoming => "incoming",
        TrailDirection::Outgoing => "outgoing",
        TrailDirection::Both => "both",
    }
}

#[cfg(test)]
mod trail_story_tests {
    use super::*;
    use codestory_contracts::api::{EdgeId, LayoutDirection};

    fn node(id: &str, label: &str, file_path: &str) -> GraphNodeDto {
        GraphNodeDto {
            id: NodeId(id.to_string()),
            label: label.to_string(),
            kind: NodeKind::FUNCTION,
            depth: 0,
            label_policy: None,
            badge_visible_members: None,
            badge_total_members: None,
            merged_symbol_examples: Vec::new(),
            file_path: Some(file_path.to_string()),
            qualified_name: None,
            member_access: None,
        }
    }

    fn edge(id: usize, source: &str, target: &str, certainty: Option<&str>) -> GraphEdgeDto {
        GraphEdgeDto {
            id: EdgeId(format!("edge-{id}")),
            source: NodeId(source.to_string()),
            target: NodeId(target.to_string()),
            kind: EdgeKind::CALL,
            confidence: Some(0.99),
            certainty: certainty.map(ToOwned::to_owned),
            callsite_identity: None,
            candidate_targets: Vec::new(),
        }
    }

    fn request(story: bool) -> TrailConfigDto {
        TrailConfigDto {
            root_id: NodeId("focus".to_string()),
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 2,
            direction: TrailDirection::Both,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: Vec::new(),
            show_utility_calls: false,
            hide_speculative: false,
            story,
            node_filter: Vec::new(),
            max_nodes: 24,
            layout_direction: LayoutDirection::Horizontal,
        }
    }

    fn focus_details() -> NodeDetailsDto {
        NodeDetailsDto {
            id: NodeId("focus".to_string()),
            kind: NodeKind::FUNCTION,
            display_name: "handle_request".to_string(),
            serialized_name: "handle_request".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: Some("C:/repo/src/request.rs".to_string()),
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            member_access: None,
        }
    }

    #[test]
    fn trail_story_preserves_missing_certainty_and_reports_story_truncation() {
        let focus = focus_details();
        let mut nodes = vec![node("focus", "handle_request", "C:/repo/src/request.rs")];
        let mut edges = Vec::new();
        for index in 0..18 {
            let target = format!("target-{index}");
            nodes.push(node(
                &target,
                &format!("target_{index}"),
                "C:/repo/src/flow.rs",
            ));
            edges.push(edge(index, "focus", &target, None));
        }
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes,
            edges,
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert_eq!(story.core_flow.len(), TRAIL_STORY_CORE_FLOW_LIMIT);
        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("missing certainty metadata")),
            "missing certainty should remain textual uncertainty: {story:#?}"
        );
        assert!(
            story
                .limits
                .iter()
                .any(|item| item.contains("core_flow shows first 16 of 18 rendered edges")),
            "story-level truncation should be disclosed: {story:#?}"
        );
    }

    #[test]
    fn trail_story_reports_certainty_spectrum_textually() {
        let focus = focus_details();
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node("focus", "handle_request", "C:/repo/src/request.rs"),
                node("certain", "validate_request", "C:/repo/src/request.rs"),
                node("probable", "load_profile", "C:/repo/src/profile.rs"),
                node(
                    "speculative",
                    "dynamic_plugin_hook",
                    "C:/repo/src/plugin.rs",
                ),
                node("missing", "legacy_dispatch", "C:/repo/src/legacy.rs"),
            ],
            edges: vec![
                edge(1, "focus", "certain", Some("certain")),
                edge(2, "focus", "probable", Some("probable")),
                edge(3, "focus", "speculative", Some("speculative")),
                edge(4, "focus", "missing", None),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));
        let core_certainties = story
            .core_flow
            .iter()
            .map(|step| step.certainty.as_str())
            .collect::<Vec<_>>();

        assert!(
            core_certainties.contains(&"certain")
                && core_certainties.contains(&"probable")
                && core_certainties.contains(&"speculative")
                && core_certainties.contains(&"missing certainty metadata"),
            "core flow should keep every certainty label textual: {story:#?}"
        );
        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("speculative"))
                && story
                    .uncertainty
                    .iter()
                    .any(|item| item.contains("missing certainty metadata")),
            "uncertainty section should call out speculative and missing certainty: {story:#?}"
        );
    }

    #[test]
    fn trail_story_empty_edges_do_not_claim_certainty() {
        let focus = focus_details();
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![node("focus", "handle_request", "C:/repo/src/request.rs")],
            edges: Vec::new(),
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert!(
            story
                .uncertainty
                .iter()
                .any(|item| item.contains("no rendered trail edges to evaluate")),
            "empty story should not claim all edges are certain: {story:#?}"
        );
    }

    #[test]
    fn trail_story_side_effects_are_conservative_candidates() {
        let focus = NodeDetailsDto {
            id: NodeId("focus".to_string()),
            kind: NodeKind::FUNCTION,
            display_name: "handle_request".to_string(),
            serialized_name: "handle_request".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: None,
            start_line: None,
            start_col: None,
            end_line: None,
            end_col: None,
            member_access: None,
        };
        let trail = GraphResponse {
            center_id: NodeId("focus".to_string()),
            nodes: vec![
                node("focus", "handle_request", "C:/repo/src/request.rs"),
                node("write", "write_audit_log", "C:/repo/src/audit.rs"),
                node("catalog", "catalog_entries", "C:/repo/src/catalog.rs"),
            ],
            edges: vec![
                edge(1, "focus", "write", Some("certain")),
                edge(2, "focus", "catalog", Some("certain")),
            ],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        };

        let story = build_trail_story(None, &focus, &trail, &request(true));

        assert!(
            story
                .side_effects
                .iter()
                .any(|item| item.contains("possible side-effect candidate")
                    && item.contains("write_audit_log")),
            "write target should be flagged as a candidate: {story:#?}"
        );
        assert!(
            story
                .side_effects
                .iter()
                .all(|item| !item.contains("catalog_entries")),
            "catalog substring should not be treated as a side effect: {story:#?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{
        Edge, EdgeId, EdgeKind, Node, NodeId as CoreNodeId, NodeKind, Occurrence, OccurrenceKind,
        SourceLocation,
    };
    use tempfile::tempdir;

    fn insert_file_node(
        storage: &mut Storage,
        file_id: i64,
        path: &Path,
        child: Node,
    ) -> Result<(), Box<dyn std::error::Error>> {
        storage.insert_file(&FileInfo {
            id: file_id,
            path: path.to_path_buf(),
            language: "rust".to_string(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 10,
        })?;
        storage.insert_nodes_batch(&[
            Node {
                id: CoreNodeId(file_id),
                kind: NodeKind::FILE,
                serialized_name: path.to_string_lossy().to_string(),
                ..Default::default()
            },
            child,
        ])?;
        Ok(())
    }

    #[test]
    fn grounding_snapshot_represents_all_files() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let first = temp.path().join("src").join("lib.rs");
            let second = temp.path().join("src").join("main.rs");
            std::fs::create_dir_all(first.parent().expect("first parent")).expect("create src");
            std::fs::write(&first, "fn alpha() {}\n").expect("write first");
            std::fs::write(&second, "fn beta() {}\n").expect("write second");
            insert_file_node(
                &mut storage,
                11,
                &first,
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "alpha".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    ..Default::default()
                },
            )
            .expect("insert first");
            insert_file_node(
                &mut storage,
                12,
                &second,
                Node {
                    id: CoreNodeId(102),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "beta".to_string(),
                    file_node_id: Some(CoreNodeId(12)),
                    start_line: Some(1),
                    ..Default::default()
                },
            )
            .expect("insert second");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("grounding snapshot");

        assert_eq!(snapshot.coverage.total_files, 2);
        assert_eq!(snapshot.coverage.represented_files, 2);
        assert_eq!(snapshot.files.len(), 2);
        assert!(snapshot.coverage_buckets.is_empty());
    }

    #[test]
    fn grounding_snapshot_caps_detailed_files_and_adds_coverage_buckets() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            for index in 0..10 {
                let path = temp.path().join("src").join(format!("module_{index}.rs"));
                std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
                std::fs::write(&path, format!("fn symbol_{index}() {{}}\n")).expect("write file");
                insert_file_node(
                    &mut storage,
                    11 + index,
                    &path,
                    Node {
                        id: CoreNodeId(101 + index),
                        kind: NodeKind::FUNCTION,
                        serialized_name: format!("symbol_{index}"),
                        file_node_id: Some(CoreNodeId(11 + index)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                )
                .expect("insert file");
            }
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("grounding snapshot");

        assert_eq!(snapshot.coverage.total_files, 10);
        assert_eq!(snapshot.coverage.represented_files, 10);
        assert_eq!(snapshot.files.len(), 8);
        assert_eq!(
            snapshot
                .coverage_buckets
                .iter()
                .map(|bucket| bucket.file_count)
                .sum::<u32>(),
            2
        );
    }

    #[test]
    fn grounding_snapshot_deprioritizes_import_like_root_symbols() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let path = temp.path().join("src").join("lib.rs");
            std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
            std::fs::write(&path, "class Widget {}\n").expect("write file");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: path.clone(),
                    language: "rust".to_string(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 10,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::MODULE,
                        serialized_name: "\"./random.js\"".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::CLASS,
                        serialized_name: "Widget".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(2),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("grounding snapshot");

        assert!(
            snapshot
                .root_symbols
                .first()
                .is_some_and(|symbol| symbol.label.starts_with("Widget"))
        );
    }

    #[test]
    fn grounding_snapshot_represented_symbols_is_monotonic_across_budgets() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            for file_index in 0..24 {
                let path = temp
                    .path()
                    .join("src")
                    .join(format!("module_{file_index}.rs"));
                std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
                std::fs::write(&path, format!("fn symbol_{file_index}_0() {{}}\n"))
                    .expect("write file");

                let file_id = 500 + file_index;
                let file_node_id = CoreNodeId(file_id);
                storage
                    .insert_file(&FileInfo {
                        id: file_id,
                        path: path.clone(),
                        language: "rust".to_string(),
                        modification_time: 0,
                        indexed: true,
                        complete: true,
                        line_count: 10,
                    })
                    .expect("insert file");
                storage
                    .insert_nodes_batch(&[
                        Node {
                            id: file_node_id,
                            kind: NodeKind::FILE,
                            serialized_name: path.to_string_lossy().to_string(),
                            ..Default::default()
                        },
                        Node {
                            id: CoreNodeId(5_000 + file_index * 10),
                            kind: NodeKind::STRUCT,
                            serialized_name: format!("Controller{file_index}"),
                            file_node_id: Some(file_node_id),
                            start_line: Some(1),
                            ..Default::default()
                        },
                        Node {
                            id: CoreNodeId(5_001 + file_index * 10),
                            kind: NodeKind::FUNCTION,
                            serialized_name: format!("check_winner_{file_index}"),
                            file_node_id: Some(file_node_id),
                            start_line: Some(2),
                            ..Default::default()
                        },
                        Node {
                            id: CoreNodeId(5_002 + file_index * 10),
                            kind: NodeKind::FUNCTION,
                            serialized_name: format!("min_max_{file_index}"),
                            file_node_id: Some(file_node_id),
                            start_line: Some(3),
                            ..Default::default()
                        },
                        Node {
                            id: CoreNodeId(5_003 + file_index * 10),
                            kind: NodeKind::FUNCTION,
                            serialized_name: format!("helper_{file_index}"),
                            file_node_id: Some(file_node_id),
                            start_line: Some(4),
                            ..Default::default()
                        },
                        Node {
                            id: CoreNodeId(5_004 + file_index * 10),
                            kind: NodeKind::FUNCTION,
                            serialized_name: format!("extra_{file_index}"),
                            file_node_id: Some(file_node_id),
                            start_line: Some(5),
                            ..Default::default()
                        },
                    ])
                    .expect("insert nodes");
            }
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let strict = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("strict snapshot");
        let balanced = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("balanced snapshot");
        let max = controller
            .grounding_snapshot(GroundingBudgetDto::Max)
            .expect("max snapshot");

        assert!(strict.coverage.represented_symbols <= balanced.coverage.represented_symbols);
        assert!(balanced.coverage.represented_symbols <= max.coverage.represented_symbols);
        assert!(strict.files.len() <= balanced.files.len());
        assert!(balanced.files.len() <= max.files.len());

        for snapshot in [&strict, &balanced, &max] {
            let surfaced_symbols = snapshot
                .files
                .iter()
                .map(|file| file.symbol_count)
                .sum::<u32>()
                .saturating_add(
                    snapshot
                        .coverage_buckets
                        .iter()
                        .map(|bucket| bucket.symbol_count)
                        .sum::<u32>(),
                );
            assert_eq!(snapshot.coverage.represented_symbols, surfaced_symbols);
        }
    }

    #[test]
    fn grounding_snapshot_batches_member_counts_line_fallbacks_and_edge_digests() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let path = temp.path().join("src").join("lib.rs");
            std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
            std::fs::write(&path, "struct Controller { value: i32 }\n").expect("write file");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: path.clone(),
                    language: "rust".to_string(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 10,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::STRUCT,
                        serialized_name: "Controller".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::FIELD,
                        serialized_name: "value".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(4),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[Edge {
                    id: EdgeId(501),
                    source: CoreNodeId(101),
                    target: CoreNodeId(102),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(11)),
                    line: Some(3),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }])
                .expect("insert edges");
            storage
                .insert_occurrences_batch(&[Occurrence {
                    element_id: 101,
                    kind: OccurrenceKind::DEFINITION,
                    location: SourceLocation {
                        file_node_id: CoreNodeId(11),
                        start_line: 3,
                        start_col: 1,
                        end_line: 3,
                        end_col: 10,
                    },
                }])
                .expect("insert occurrences");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("grounding snapshot");

        let symbol = snapshot
            .root_symbols
            .iter()
            .find(|symbol| symbol.label.starts_with("Controller"))
            .expect("controller root symbol");
        assert_eq!(symbol.line, Some(3));
        assert_eq!(symbol.member_count, Some(1));
        assert!(symbol.edge_digest.iter().any(|digest| digest == "MEMBER=1"));
    }

    #[test]
    fn grounding_snapshot_uses_materialized_snapshot_after_summary_open() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let path = temp.path().join("src").join("lib.rs");
            std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
            std::fs::write(&path, "struct Controller {}\nfn helper() {}\n").expect("write file");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: path.clone(),
                    language: "rust".to_string(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 10,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::STRUCT,
                        serialized_name: "Controller".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::FUNCTION,
                        serialized_name: "helper".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(2),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .snapshots()
                .refresh_all()
                .expect("refresh grounding snapshots");
            assert!(
                storage
                    .snapshots()
                    .has_ready_summary()
                    .expect("summary snapshot readiness"),
                "expected ready grounding summary snapshot after refresh"
            );
            assert!(
                storage
                    .snapshots()
                    .has_ready_detail()
                    .expect("detail snapshot readiness"),
                "expected ready grounding detail snapshot after refresh"
            );
        }

        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project summary");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("grounding snapshot");

        assert_eq!(snapshot.coverage.total_files, 1);
        assert_eq!(snapshot.files.len(), 1);
        assert!(
            snapshot
                .root_symbols
                .iter()
                .any(|symbol| symbol.label.starts_with("Controller")),
            "expected materialized root symbol to be surfaced"
        );
    }

    #[test]
    fn balanced_grounding_falls_back_to_live_detail_queries_when_detail_tier_is_dirty() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let path = temp.path().join("src").join("lib.rs");
            std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
            std::fs::write(&path, "struct Controller { value: i32 }\n").expect("write file");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: path.clone(),
                    language: "rust".to_string(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 10,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::STRUCT,
                        serialized_name: "Controller".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::FIELD,
                        serialized_name: "value".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(4),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[Edge {
                    id: EdgeId(501),
                    source: CoreNodeId(101),
                    target: CoreNodeId(102),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(11)),
                    line: Some(3),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }])
                .expect("insert edges");
            storage
                .insert_occurrences_batch(&[Occurrence {
                    element_id: 101,
                    kind: OccurrenceKind::DEFINITION,
                    location: SourceLocation {
                        file_node_id: CoreNodeId(11),
                        start_line: 3,
                        start_col: 1,
                        end_line: 3,
                        end_col: 10,
                    },
                }])
                .expect("insert occurrences");
            storage
                .snapshots()
                .refresh_summary()
                .expect("refresh summary snapshots");
            assert!(
                storage
                    .snapshots()
                    .has_ready_summary()
                    .expect("summary snapshot readiness"),
                "expected ready grounding summary snapshots"
            );
            assert!(
                !storage
                    .snapshots()
                    .has_ready_detail()
                    .expect("detail snapshot readiness"),
                "expected detail snapshots to stay dirty"
            );
        }

        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(temp.path().to_path_buf(), db_path.clone())
            .expect("open project summary");

        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("balanced snapshot");
        let symbol = snapshot
            .root_symbols
            .iter()
            .find(|symbol| symbol.label.starts_with("Controller"))
            .expect("controller root symbol");
        assert_eq!(symbol.line, Some(3));
        assert_eq!(symbol.member_count, Some(1));
        assert!(symbol.edge_digest.iter().any(|digest| digest == "MEMBER=1"));

        let storage = Storage::open(&db_path).expect("reopen storage");
        assert!(
            !storage
                .snapshots()
                .has_ready_detail()
                .expect("detail snapshot readiness"),
            "balanced should not eagerly hydrate detail snapshots"
        );
    }

    #[test]
    fn max_grounding_hydrates_detail_snapshots_when_unavailable() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let path = temp.path().join("src").join("lib.rs");
            std::fs::create_dir_all(path.parent().expect("path parent")).expect("create src");
            std::fs::write(&path, "struct Controller { value: i32 }\n").expect("write file");
            storage
                .insert_file(&FileInfo {
                    id: 11,
                    path: path.clone(),
                    language: "rust".to_string(),
                    modification_time: 0,
                    indexed: true,
                    complete: true,
                    line_count: 10,
                })
                .expect("insert file");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(11),
                        kind: NodeKind::FILE,
                        serialized_name: path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(101),
                        kind: NodeKind::STRUCT,
                        serialized_name: "Controller".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(102),
                        kind: NodeKind::FIELD,
                        serialized_name: "value".to_string(),
                        file_node_id: Some(CoreNodeId(11)),
                        start_line: Some(4),
                        ..Default::default()
                    },
                ])
                .expect("insert nodes");
            storage
                .insert_edges_batch(&[Edge {
                    id: EdgeId(501),
                    source: CoreNodeId(101),
                    target: CoreNodeId(102),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(11)),
                    line: Some(3),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                }])
                .expect("insert edges");
            storage
                .snapshots()
                .refresh_summary()
                .expect("refresh summary snapshots");
        }

        let controller = AppController::new();
        controller
            .open_project_summary_with_storage_path(temp.path().to_path_buf(), db_path.clone())
            .expect("open project summary");
        controller
            .grounding_snapshot(GroundingBudgetDto::Max)
            .expect("max snapshot");

        let storage = Storage::open(&db_path).expect("reopen storage");
        assert!(
            storage
                .snapshots()
                .has_ready_detail()
                .expect("detail snapshot readiness"),
            "max should hydrate detail snapshots when needed"
        );
    }
}
