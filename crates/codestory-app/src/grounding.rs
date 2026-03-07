use super::*;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

fn is_import_like_symbol(node: &codestory_core::Node) -> bool {
    matches!(
        node.kind,
        codestory_core::NodeKind::MODULE
            | codestory_core::NodeKind::NAMESPACE
            | codestory_core::NodeKind::PACKAGE
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

fn node_rank(node: &codestory_core::Node) -> u8 {
    if is_import_like_symbol(node) {
        return 5;
    }

    match node.kind {
        codestory_core::NodeKind::CLASS
        | codestory_core::NodeKind::STRUCT
        | codestory_core::NodeKind::INTERFACE
        | codestory_core::NodeKind::ENUM
        | codestory_core::NodeKind::UNION
        | codestory_core::NodeKind::ANNOTATION
        | codestory_core::NodeKind::TYPEDEF => 0,
        codestory_core::NodeKind::FUNCTION
        | codestory_core::NodeKind::METHOD
        | codestory_core::NodeKind::MACRO => 1,
        codestory_core::NodeKind::MODULE
        | codestory_core::NodeKind::NAMESPACE
        | codestory_core::NodeKind::PACKAGE => 2,
        codestory_core::NodeKind::FIELD
        | codestory_core::NodeKind::VARIABLE
        | codestory_core::NodeKind::GLOBAL_VARIABLE
        | codestory_core::NodeKind::CONSTANT
        | codestory_core::NodeKind::ENUM_CONSTANT
        | codestory_core::NodeKind::TYPE_PARAMETER => 3,
        _ => 4,
    }
}

fn compare_nodes(left: &codestory_core::Node, right: &codestory_core::Node) -> Ordering {
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
    file: codestory_storage::FileInfo,
    relative_path: String,
    nodes: Vec<codestory_core::Node>,
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
    storage: &Storage,
    root: &Path,
    node: &codestory_core::Node,
) -> Result<GroundingSymbolDigestDto, ApiError> {
    let display_name = node_display_name(node);
    let member_count = if is_structural_kind(node.kind) {
        Some(
            storage
                .get_children_symbols(node.id)
                .map_err(|e| ApiError::internal(format!("Failed to load child symbols: {e}")))?
                .len()
                .min(u32::MAX as usize) as u32,
        )
    } else {
        None
    };

    let line = node.start_line.or_else(|| {
        storage
            .get_occurrences_for_node(node.id)
            .ok()
            .and_then(|occurrences| {
                occurrences
                    .first()
                    .map(|occurrence| occurrence.location.start_line)
            })
    });

    let label = if let Some(file_path) = AppController::file_path_for_node(storage, node)? {
        let path = Path::new(&file_path);
        if let Ok(stripped) = path.strip_prefix(root) {
            format!(
                "{} @ {}",
                display_name,
                stripped.to_string_lossy().replace('\\', "/")
            )
        } else {
            display_name
        }
    } else {
        display_name
    };

    Ok(GroundingSymbolDigestDto {
        id: NodeId::from(node.id),
        label,
        kind: NodeKind::from(node.kind),
        line,
        member_count,
        edge_digest: edge_digest_for_node(storage, node.id, 4),
    })
}

impl AppController {
    pub fn grounding_snapshot(
        &self,
        budget: GroundingBudgetDto,
    ) -> Result<GroundingSnapshotDto, ApiError> {
        let root = self.require_project_root()?;
        let storage = self.open_storage()?;
        let config = budget_config(budget);

        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;
        let all_nodes = storage
            .get_nodes()
            .map_err(|e| ApiError::internal(format!("Failed to load nodes: {e}")))?;
        let mut file_entries = BTreeMap::<i64, codestory_storage::FileInfo>::new();
        for file in storage
            .get_files()
            .map_err(|e| ApiError::internal(format!("Failed to load files: {e}")))?
        {
            file_entries.insert(file.id, file);
        }
        for node in &all_nodes {
            if node.kind == codestory_core::NodeKind::FILE {
                file_entries
                    .entry(node.id.0)
                    .or_insert_with(|| codestory_storage::FileInfo {
                        id: node.id.0,
                        path: PathBuf::from(&node.serialized_name),
                        language: String::new(),
                        modification_time: 0,
                        indexed: true,
                        complete: true,
                        line_count: 0,
                    });
            }
        }

        let derived_file_count = if stats.file_count > 0 {
            stats.file_count
        } else {
            file_entries.len().min(i64::MAX as usize) as i64
        };
        let dto_stats = StorageStatsDto {
            node_count: clamp_i64_to_u32(stats.node_count),
            edge_count: clamp_i64_to_u32(stats.edge_count),
            file_count: clamp_i64_to_u32(derived_file_count),
            error_count: clamp_i64_to_u32(stats.error_count),
        };

        let mut all_nodes = all_nodes;
        all_nodes.retain(|node| llm_indexable_kind(node.kind));
        all_nodes.sort_by(compare_nodes);

        let mut nodes_by_file = BTreeMap::<i64, Vec<codestory_core::Node>>::new();
        for node in all_nodes {
            if let Some(file_node_id) = node.file_node_id {
                nodes_by_file.entry(file_node_id.0).or_default().push(node);
            }
        }

        let mut files = file_entries.into_values().collect::<Vec<_>>();
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let mut file_coverages = Vec::with_capacity(files.len());
        for file in files {
            let mut file_nodes = nodes_by_file.remove(&file.id).unwrap_or_default();
            file_nodes.sort_by(compare_nodes);

            file_coverages.push(FileCoverage {
                relative_path: relative_path(&root, &file.path),
                total_symbol_count: file_nodes.len().min(u32::MAX as usize) as u32,
                represented_symbol_count: file_nodes.len().min(config.symbols_per_file) as u32,
                best_node_rank: file_nodes.first().map(node_rank).unwrap_or(u8::MAX),
                nodes: file_nodes,
                file,
            });
        }
        file_coverages.sort_by(compare_file_coverage);

        let expanded_files = file_coverages.len().min(config.expanded_files);
        let omitted_files = file_coverages.len().saturating_sub(expanded_files);

        let mut represented_symbols = 0u32;
        let mut compressed_files = omitted_files.min(u32::MAX as usize) as u32;
        let mut file_digests = Vec::with_capacity(expanded_files);
        let mut omitted_coverages = Vec::with_capacity(omitted_files);
        for (index, coverage) in file_coverages.into_iter().enumerate() {
            if index >= expanded_files {
                represented_symbols =
                    represented_symbols.saturating_add(coverage.total_symbol_count);
                omitted_coverages.push(coverage);
                continue;
            }

            let mut symbols = Vec::with_capacity(coverage.represented_symbol_count as usize);
            for node in coverage.nodes.iter().take(config.symbols_per_file) {
                symbols.push(symbol_digest(&storage, &root, node)?);
            }
            if coverage.total_symbol_count > coverage.represented_symbol_count {
                compressed_files = compressed_files.saturating_add(1);
            }
            represented_symbols =
                represented_symbols.saturating_add(coverage.represented_symbol_count);

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
        represented_symbols = represented_symbols.max(
            bucketed_symbols.saturating_add(
                file_digests
                    .iter()
                    .map(|file| file.represented_symbol_count)
                    .sum::<u32>(),
            ),
        );

        let mut roots = storage
            .get_root_symbols()
            .map_err(|e| ApiError::internal(format!("Failed to load root symbols: {e}")))?;
        roots.sort_by(compare_nodes);
        let labels_by_id = self.cached_labels(roots.iter().map(|node| node.id));
        roots = Self::dedupe_symbol_nodes(roots, &labels_by_id);

        let mut root_symbols = Vec::new();
        for node in roots.iter().take(config.root_symbols) {
            root_symbols.push(symbol_digest(&storage, &root, node)?);
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

        Ok(GroundingSnapshotDto {
            root: root.to_string_lossy().to_string(),
            budget,
            generated_at_epoch_ms: current_epoch_ms(),
            stats: dto_stats,
            coverage: codestory_api::GroundingCoverageDto {
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
    use codestory_core::{Node, NodeId as CoreNodeId, NodeKind};
    use tempfile::tempdir;

    fn insert_file_node(
        storage: &mut Storage,
        file_id: i64,
        path: &Path,
        child: Node,
    ) -> Result<(), Box<dyn std::error::Error>> {
        storage.insert_file(&codestory_storage::FileInfo {
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
                .insert_file(&codestory_storage::FileInfo {
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
}
