use super::*;
use std::cmp::Ordering;
use std::collections::BTreeMap;
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
        label,
        kind: NodeKind::from(node.kind),
        line,
        member_count,
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
        let retrieval = self.retrieval_state().ok();
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
            .search(SearchRequest {
                query: node.display_name.clone(),
            })?
            .into_iter()
            .filter(|hit| hit.node_id != node_id)
            .take(6)
            .collect();

        Ok(SymbolContextDto {
            node,
            children,
            related_hits,
            edge_digest: edge_digest_for_node(&storage, core_id, 6),
        })
    }

    pub fn trail_context(&self, req: TrailConfigDto) -> Result<TrailContextDto, ApiError> {
        let focus = self.node_details(NodeDetailsRequest {
            id: req.root_id.clone(),
        })?;
        let trail = self.graph_trail(req)?;
        Ok(TrailContextDto { focus, trail })
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
        let file = self.read_file_text(ReadFileTextRequest { path: path.clone() })?;

        Ok(SnippetContextDto {
            node,
            path: file.path,
            line,
            snippet: markdown_snippet(&file.text, Some(line), context_lines),
        })
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
