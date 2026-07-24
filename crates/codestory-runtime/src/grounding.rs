#[cfg(test)]
use super::Storage;
use super::{
    ApiError, AppController, FileInfo, GroundingBudgetDto, GroundingCoverageBucketDto,
    GroundingEdgeKindCount, GroundingFileDigestDto, GroundingNodeRecord,
    GroundingOrientationConfidenceDto, GroundingOrientationDto, GroundingOrientationUncertaintyDto,
    GroundingSnapshotDto, GroundingSymbolDigestDto, NodeDetailsRequest, NodeId, NodeKind,
    RetrievalScoreBreakdownDto, SearchHit, SnippetContextDto, StorageStatsDto, SymbolContextDto,
    SymbolSummaryRecord, TrailConfigDto, TrailContextDto, clamp_i64_to_u32, current_epoch_ms,
    edge_digest_for_node, is_structural_kind, node_display_name, normalize_symbol_query,
    retrieval_state_from_storage_for_runtime, terminal_symbol_segment,
};
use crate::agent::packet_evidence::{decorate_search_hit_evidence, diagnostic_source_evidence};
use crate::trail_story::build_trail_story;
use codestory_contracts::api::{
    PacketEvidenceResolutionDto, PacketEvidenceTierDto, SearchHitOrigin,
};
use codestory_store::{FileRole, StructuralTextUnit};
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const RECOMMENDED_QUERY_LIMIT: usize = 5;
const FUNCTION_BODY_FALLBACK_MAX_SCAN_LINES: usize = 400;
const FUNCTION_BODY_FALLBACK_BRACE_SEARCH_LINES: usize = 40;
const ROOT_CANDIDATE_MULTIPLIER: usize = 8;
const ARCHITECTURE_ROOT_FILE_LIMIT: usize = 48;
const ARCHITECTURE_ROOT_SYMBOL_SCAN_LIMIT: usize = 16;
const ARCHITECTURE_NAMED_ROOTS_PER_FILE: usize = 8;
const ARCHITECTURE_ROOT_EXACT_NAMES: &[&str] = &[
    "main",
    "run",
    "start",
    "bootstrap",
    "launch",
    "mount",
    "serve",
    "init",
    "initialize",
    "createapp",
    "createapplication",
    "createserver",
    "createrouter",
    "createruntime",
    "runapp",
    "runapplication",
    "runserver",
    "runruntime",
    "runservice",
    "runcli",
    "startapp",
    "startapplication",
    "startserver",
    "startruntime",
    "startservice",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "head",
    "options",
];
const ARCHITECTURE_ROOT_UPPERCASE_GLOBS: &[&str] =
    &["Page", "Layout", "[A-Z]*Page", "[A-Z]*Layout"];

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
    is_wrapped_import_name(trimmed) || is_relative_import_path(trimmed) || trimmed.contains('/')
}

fn is_wrapped_import_name(trimmed: &str) -> bool {
    is_surrounded_by(trimmed, '"', '"')
        || is_surrounded_by(trimmed, '\'', '\'')
        || is_surrounded_by(trimmed, '<', '>')
}

fn is_relative_import_path(trimmed: &str) -> bool {
    trimmed.starts_with("./") || trimmed.starts_with("../")
}

fn is_surrounded_by(value: &str, start: char, end: char) -> bool {
    value.starts_with(start) && value.ends_with(end)
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

struct SymbolDigestContext<'a> {
    member_counts: &'a HashMap<codestory_contracts::graph::NodeId, u32>,
    fallback_lines: &'a HashMap<codestory_contracts::graph::NodeId, u32>,
    edge_digests: &'a HashMap<codestory_contracts::graph::NodeId, Vec<String>>,
    summaries: &'a HashMap<codestory_contracts::graph::NodeId, SymbolSummaryRecord>,
    structural_units: &'a HashMap<codestory_contracts::graph::NodeId, StructuralTextUnit>,
}

fn symbol_digest(
    node: &codestory_contracts::graph::Node,
    display_name: &str,
    relative_file_path: Option<&str>,
    context: &SymbolDigestContext<'_>,
) -> GroundingSymbolDigestDto {
    let member_count = if is_structural_kind(node.kind) {
        Some(*context.member_counts.get(&node.id).unwrap_or(&0))
    } else {
        None
    };

    let line = node
        .start_line
        .or_else(|| context.fallback_lines.get(&node.id).copied());

    let label = if let Some(file_path) = relative_file_path {
        format!("{display_name} @ {file_path}")
    } else {
        display_name.to_string()
    };
    let diagnostic_evidence =
        diagnostic_source_evidence(relative_file_path, node.canonical_id.as_deref());
    let structural_unit = context.structural_units.get(&node.id);

    GroundingSymbolDigestDto {
        id: NodeId::from(node.id),
        node_ref: relative_file_path
            .zip(line)
            .map(|(path, line)| format!("{path}:{line}:{display_name}")),
        label,
        kind: NodeKind::from(node.kind),
        line,
        member_count,
        summary: context
            .summaries
            .get(&node.id)
            .map(|record| record.summary.clone()),
        edge_digest: context
            .edge_digests
            .get(&node.id)
            .cloned()
            .unwrap_or_default(),
        evidence_tier: structural_unit
            .map(|_| PacketEvidenceTierDto::StructuralText)
            .or_else(|| diagnostic_evidence.map(|evidence| evidence.tier)),
        evidence_producer: structural_unit
            .map(|unit| unit.producer.clone())
            .or_else(|| diagnostic_evidence.map(|evidence| evidence.producer.to_string())),
        resolution_status: structural_unit
            .map(|_| PacketEvidenceResolutionDto::SourceRangeOnly)
            .or_else(|| diagnostic_evidence.map(|evidence| evidence.resolution)),
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

fn is_production_file_role(role: Option<FileRole>) -> bool {
    matches!(role, Some(FileRole::Source | FileRole::Entrypoint))
}

fn grounding_root_file_role_rank(role: Option<FileRole>) -> u8 {
    match role {
        Some(FileRole::Entrypoint) => 0,
        Some(FileRole::Source) => 1,
        _ => 2,
    }
}

fn grounding_root_terminal_name(record: &GroundingNodeRecord) -> String {
    let terminal = terminal_symbol_segment(&record.display_name);
    if terminal.is_empty() {
        normalize_symbol_query(&record.display_name)
    } else {
        terminal
    }
}

fn grounding_root_path_rank(root: &Path, record: &GroundingNodeRecord) -> u8 {
    record
        .file_path
        .as_deref()
        .map(|path| relative_path(root, path))
        .as_deref()
        .map_or(3, |path| architecture_path_rank(Some(path)))
}

fn grounding_root_subsystem_key(
    root: &Path,
    record: &GroundingNodeRecord,
    file_languages: &HashMap<i64, String>,
) -> String {
    let language = record
        .node
        .file_node_id
        .and_then(|file_id| file_languages.get(&file_id.0))
        .map(String::as_str)
        .unwrap_or("unknown");
    let Some(path) = record.file_path.as_deref() else {
        return format!("{language}:unknown");
    };
    let relative = relative_path(root, path).to_ascii_lowercase();
    let segments = relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if let Some(index) = segments.iter().position(|segment| *segment == "crates")
        && let Some(crate_name) = segments.get(index + 1)
    {
        return format!("{language}:crates/{crate_name}");
    }
    if let Some(index) = segments.iter().position(|segment| *segment == "plugins")
        && let Some(plugin_name) = segments.get(index + 1)
    {
        return format!("{language}:plugins/{plugin_name}");
    }
    if segments.contains(&"src-tauri") {
        return format!("{language}:src-tauri");
    }
    if let Some(index) = segments.iter().rposition(|segment| *segment == "src") {
        if let Some(next) = segments.get(index + 1)
            && !next.contains('.')
        {
            return format!("{language}:{}", segments[..=index + 1].join("/"));
        }
        return format!("{language}:{}", segments[..=index].join("/"));
    }

    let top = segments.first().copied().unwrap_or("root");
    format!("{language}:{top}")
}

fn build_grounding_edge_degree_map(
    counts: Vec<GroundingEdgeKindCount>,
) -> HashMap<codestory_contracts::graph::NodeId, u32> {
    let mut degrees = HashMap::new();
    for count in counts {
        degrees
            .entry(count.node_id)
            .and_modify(|total: &mut u32| *total = total.saturating_add(count.count))
            .or_insert(count.count);
    }
    degrees
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GroundingRootSortKey {
    import_like: bool,
    entrypoint: Reverse<bool>,
    file_role_rank: u8,
    path_rank: u8,
    edge_degree: Reverse<u32>,
    member_count: Reverse<u32>,
    node_rank: u8,
    start_line: u32,
    relative_path: Option<String>,
    display_name: String,
    node_id: i64,
}

impl GroundingRootSortKey {
    fn new(
        record: &GroundingNodeRecord,
        root: &Path,
        file_roles: &HashMap<i64, FileRole>,
        edge_degrees: &HashMap<codestory_contracts::graph::NodeId, u32>,
        member_counts: &HashMap<codestory_contracts::graph::NodeId, u32>,
    ) -> Self {
        let role = record
            .node
            .file_node_id
            .and_then(|file_id| file_roles.get(&file_id.0).copied());
        Self {
            import_like: is_import_like_symbol(&record.node),
            entrypoint: Reverse(is_grounding_entrypoint_root(root, record, file_roles)),
            file_role_rank: grounding_root_file_role_rank(role),
            path_rank: grounding_root_path_rank(root, record),
            edge_degree: Reverse(edge_degrees.get(&record.node.id).copied().unwrap_or(0)),
            member_count: Reverse(member_counts.get(&record.node.id).copied().unwrap_or(0)),
            node_rank: node_rank(&record.node),
            start_line: record.node.start_line.unwrap_or(u32::MAX),
            relative_path: record
                .file_path
                .as_deref()
                .map(|path| relative_path(root, path)),
            display_name: record.display_name.clone(),
            node_id: record.node.id.0,
        }
    }
}

fn append_diversified_grounding_root_tier(
    records: Vec<GroundingNodeRecord>,
    root: &Path,
    file_languages: &HashMap<i64, String>,
    seen_surfaces: &mut HashSet<String>,
    seen_names: &mut HashSet<String>,
    diversified: &mut Vec<GroundingNodeRecord>,
) {
    let mut repeated_surfaces = Vec::new();
    for record in records {
        let surface = grounding_root_subsystem_key(root, &record, file_languages);
        let name = grounding_root_terminal_name(&record);
        if !seen_surfaces.contains(&surface) && !seen_names.contains(&name) {
            seen_surfaces.insert(surface);
            seen_names.insert(name);
            diversified.push(record);
        } else {
            repeated_surfaces.push(record);
        }
    }

    let mut duplicate_names = Vec::new();
    for record in repeated_surfaces {
        if seen_names.insert(grounding_root_terminal_name(&record)) {
            diversified.push(record);
        } else {
            duplicate_names.push(record);
        }
    }
    diversified.extend(duplicate_names);
}

fn diversify_grounding_root_records(
    mut records: Vec<GroundingNodeRecord>,
    root: &Path,
    file_roles: &HashMap<i64, FileRole>,
    file_languages: &HashMap<i64, String>,
    edge_degrees: &HashMap<codestory_contracts::graph::NodeId, u32>,
    member_counts: &HashMap<codestory_contracts::graph::NodeId, u32>,
) -> Vec<GroundingNodeRecord> {
    records.sort_by_cached_key(|record| {
        GroundingRootSortKey::new(record, root, file_roles, edge_degrees, member_counts)
    });
    let (production, secondary): (Vec<_>, Vec<_>) = records.into_iter().partition(|record| {
        is_production_file_role(
            record
                .node
                .file_node_id
                .and_then(|file_id| file_roles.get(&file_id.0).copied()),
        )
    });

    // Spend the compact budget on distinct production language/subsystem
    // surfaces, then distinct names, before compatibility-only candidates.
    // Every budget truncates this one stable order.
    let mut seen_surfaces = HashSet::new();
    let mut seen_names = HashSet::new();
    let mut diversified = Vec::with_capacity(production.len() + secondary.len());
    append_diversified_grounding_root_tier(
        production,
        root,
        file_languages,
        &mut seen_surfaces,
        &mut seen_names,
        &mut diversified,
    );
    append_diversified_grounding_root_tier(
        secondary,
        root,
        file_languages,
        &mut seen_surfaces,
        &mut seen_names,
        &mut diversified,
    );
    diversified
}

fn is_grounding_entrypoint_root(
    root: &Path,
    record: &GroundingNodeRecord,
    file_roles: &HashMap<i64, FileRole>,
) -> bool {
    let role = record
        .node
        .file_node_id
        .and_then(|file_id| file_roles.get(&file_id.0).copied());
    if is_import_like_symbol(&record.node)
        || !is_production_file_role(role)
        || !matches!(
            record.node.kind,
            codestory_contracts::graph::NodeKind::FUNCTION
                | codestory_contracts::graph::NodeKind::METHOD
        )
    {
        return false;
    }
    let has_entrypoint_file_evidence =
        role == Some(FileRole::Entrypoint) || grounding_root_path_rank(root, record) == 0;
    if !has_entrypoint_file_evidence {
        return false;
    }

    let name = grounding_root_terminal_name(record)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    if [
        "main",
        "run",
        "start",
        "bootstrap",
        "launch",
        "mount",
        "serve",
        "init",
        "initialize",
        "createapp",
        "createapplication",
        "get",
        "post",
        "put",
        "patch",
        "delete",
        "head",
        "options",
    ]
    .iter()
    .any(|candidate| name == *candidate)
    {
        return true;
    }

    [
        (
            "start",
            &["app", "application", "server", "runtime", "service"][..],
        ),
        (
            "run",
            &["app", "application", "server", "runtime", "service", "cli"][..],
        ),
        (
            "create",
            &["app", "application", "server", "router", "runtime"][..],
        ),
    ]
    .iter()
    .any(|(prefix, suffixes)| {
        name.strip_prefix(prefix)
            .is_some_and(|suffix| suffixes.contains(&suffix))
    }) || {
        let terminal = record
            .display_name
            .rsplit([':', '.', '/', '\\'])
            .next()
            .unwrap_or_default()
            .trim();
        terminal
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_uppercase())
            && (name.ends_with("page") || name.ends_with("layout"))
    }
}

fn grounding_orientation(
    root: &Path,
    evaluated: &[GroundingNodeRecord],
    selected: &[GroundingNodeRecord],
    total_root_candidates: usize,
    compressed_files: u32,
    file_roles: &HashMap<i64, FileRole>,
    file_languages: &HashMap<i64, String>,
) -> GroundingOrientationDto {
    let candidate_entrypoint_roots = evaluated
        .iter()
        .filter(|record| is_grounding_entrypoint_root(root, record, file_roles))
        .count();
    let selected_entrypoint_roots = selected
        .iter()
        .filter(|record| is_grounding_entrypoint_root(root, record, file_roles))
        .count();
    let candidate_subsystems = evaluated
        .iter()
        .filter(|record| {
            is_production_file_role(
                record
                    .node
                    .file_node_id
                    .and_then(|file_id| file_roles.get(&file_id.0).copied()),
            )
        })
        .map(|record| grounding_root_subsystem_key(root, record, file_languages))
        .collect::<HashSet<_>>()
        .len();
    let selected_subsystems = selected
        .iter()
        .filter(|record| {
            is_production_file_role(
                record
                    .node
                    .file_node_id
                    .and_then(|file_id| file_roles.get(&file_id.0).copied()),
            )
        })
        .map(|record| grounding_root_subsystem_key(root, record, file_languages))
        .collect::<HashSet<_>>()
        .len();

    let mut uncertainty = Vec::new();
    if evaluated.len() < total_root_candidates {
        uncertainty.push(GroundingOrientationUncertaintyDto::BoundedCandidateWindow);
    }
    if candidate_entrypoint_roots == 0 {
        uncertainty.push(GroundingOrientationUncertaintyDto::NoEntrypointEvidence);
    } else if selected_entrypoint_roots == 0 {
        uncertainty.push(GroundingOrientationUncertaintyDto::EntrypointEvidenceOmitted);
    }
    if candidate_subsystems > 1 && selected_subsystems < candidate_subsystems.min(selected.len()) {
        uncertainty.push(GroundingOrientationUncertaintyDto::LimitedSubsystemBreadth);
    }
    if compressed_files > 0 {
        uncertainty.push(GroundingOrientationUncertaintyDto::CompressedPresentation);
    }

    let confidence = if selected.is_empty()
        || candidate_entrypoint_roots == 0
        || (candidate_subsystems > 1 && selected_subsystems <= 1)
    {
        GroundingOrientationConfidenceDto::Weak
    } else if evaluated.len() < total_root_candidates
        || selected_entrypoint_roots == 0
        || (candidate_subsystems > 1 && selected_subsystems < 2)
    {
        GroundingOrientationConfidenceDto::Partial
    } else {
        GroundingOrientationConfidenceDto::Strong
    };

    GroundingOrientationDto {
        confidence,
        total_root_candidates: total_root_candidates.min(u32::MAX as usize) as u32,
        evaluated_root_candidates: evaluated.len().min(u32::MAX as usize) as u32,
        candidate_entrypoint_roots: candidate_entrypoint_roots.min(u32::MAX as usize) as u32,
        selected_entrypoint_roots: selected_entrypoint_roots.min(u32::MAX as usize) as u32,
        candidate_subsystems: candidate_subsystems.min(u32::MAX as usize) as u32,
        selected_subsystems: selected_subsystems.min(u32::MAX as usize) as u32,
        uncertainty,
    }
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

#[derive(Debug)]
struct RecommendationCandidate<'a> {
    symbol: &'a GroundingSymbolDigestDto,
    name: String,
    path: Option<String>,
    order: usize,
}

fn grounding_symbol_name(symbol: &GroundingSymbolDigestDto) -> String {
    symbol
        .label
        .split(" @ ")
        .next()
        .unwrap_or(symbol.label.as_str())
        .trim()
        .trim_matches(['`', '"', '\''])
        .to_string()
}

fn grounding_symbol_path(symbol: &GroundingSymbolDigestDto) -> Option<String> {
    if let Some(node_ref) = symbol.node_ref.as_deref()
        && let Some((path, _line)) = split_grounding_node_ref_location(node_ref)
    {
        return path;
    }

    symbol
        .label
        .split_once(" @ ")
        .map(|(_, path)| path.trim().to_string())
        .filter(|path| !path.is_empty())
}

fn split_grounding_node_ref_location(value: &str) -> Option<(Option<String>, Option<u32>)> {
    let mut parts = value.rsplitn(3, ':');
    let _name = parts.next()?;
    let line = parts.next()?.parse::<u32>().ok();
    let path = parts.next().map(ToOwned::to_owned);
    Some((path, line))
}

fn normalized_recommendation_key(name: &str) -> String {
    name.rsplit([':', '.', '/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .to_ascii_lowercase()
}

fn low_value_recommendation_path(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let path = format!("/{}", path.replace('\\', "/").to_ascii_lowercase());
    [
        "/tests/",
        "/test/",
        "/testing/",
        "/fixtures/",
        "/fixture/",
        "/examples/",
        "/example/",
        "/benches/",
        "/bench/",
        "/target/",
        "/dist/",
        "/build/",
        "/migrations/",
        "/bin/app/user/projects/",
        "/src/external/",
        "/external/",
        "/vendor/",
        "/third_party/",
        "/third-party/",
    ]
    .iter()
    .any(|marker| path.contains(marker))
        || path.contains("/scripts/")
}

fn low_value_recommendation_candidate(candidate: &RecommendationCandidate<'_>) -> bool {
    low_value_recommendation_path(candidate.path.as_deref())
        || low_value_recommendation_name(&candidate.name)
}

fn low_value_recommendation_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    normalized.starts_with("std::") || normalized.starts_with("std.")
}

fn architecture_path_rank(path: Option<&str>) -> u8 {
    let Some(path) = path else {
        return 3;
    };
    let path = path.replace('\\', "/").to_ascii_lowercase();
    if path.ends_with("/src/lib.rs")
        || path.ends_with("/src/main.rs")
        || path.ends_with("/src/mod.rs")
        || path == "src/lib.rs"
        || path == "src/main.rs"
        || path.ends_with("/main.ts")
        || path.ends_with("/main.tsx")
        || path.ends_with("/main.js")
        || path.ends_with("/main.jsx")
        || path.ends_with("/app.svelte")
        || path.ends_with("/page.tsx")
        || path.ends_with("/layout.tsx")
        || path.ends_with("/route.ts")
        || path.ends_with("payload.config.ts")
        || path.ends_with("next.config.ts")
    {
        return 0;
    }
    if path.contains("/src/app/")
        || path.contains("/src/collections/")
        || path.contains("/src/components/")
        || path.contains("/src/runtime/")
        || path.contains("/src-tauri/src/")
        || path.contains("/src/index")
    {
        return 1;
    }
    if path.contains("/src/") || path.starts_with("src/") {
        return 2;
    }
    3
}

fn architecture_kind_rank(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE => 0,
        NodeKind::FUNCTION | NodeKind::METHOD => 1,
        NodeKind::TYPEDEF => 2,
        NodeKind::FIELD | NodeKind::VARIABLE | NodeKind::CONSTANT | NodeKind::ENUM_CONSTANT => 3,
        _ => 4,
    }
}

fn compare_recommendation_candidates(
    left: &RecommendationCandidate<'_>,
    right: &RecommendationCandidate<'_>,
) -> Ordering {
    low_value_recommendation_path(left.path.as_deref())
        .cmp(&low_value_recommendation_path(right.path.as_deref()))
        .then(
            architecture_path_rank(left.path.as_deref())
                .cmp(&architecture_path_rank(right.path.as_deref())),
        )
        .then(
            architecture_kind_rank(left.symbol.kind)
                .cmp(&architecture_kind_rank(right.symbol.kind)),
        )
        .then(
            right
                .symbol
                .member_count
                .unwrap_or(0)
                .cmp(&left.symbol.member_count.unwrap_or(0)),
        )
        .then(left.name.len().cmp(&right.name.len()))
        .then(left.order.cmp(&right.order))
        .then(left.name.cmp(&right.name))
}

fn recommended_grounding_queries(
    root_symbols: &[GroundingSymbolDigestDto],
    files: &[GroundingFileDigestDto],
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut order = 0usize;
    for symbol in files
        .iter()
        .flat_map(|file| file.symbols.iter())
        .chain(root_symbols.iter())
    {
        let name = grounding_symbol_name(symbol);
        if name.is_empty() || is_import_like_name(&name) {
            continue;
        }
        candidates.push(RecommendationCandidate {
            path: grounding_symbol_path(symbol),
            symbol,
            name,
            order,
        });
        order = order.saturating_add(1);
    }
    candidates.sort_by(compare_recommendation_candidates);

    let use_primary_candidates = candidates
        .iter()
        .any(|candidate| !low_value_recommendation_candidate(candidate));
    let mut seen = HashSet::new();
    let mut recommended = Vec::new();
    for candidate in candidates {
        if use_primary_candidates && low_value_recommendation_candidate(&candidate) {
            continue;
        }
        let key = normalized_recommendation_key(&candidate.name);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        recommended.push(candidate.name);
        if recommended.len() >= RECOMMENDED_QUERY_LIMIT {
            break;
        }
    }
    recommended
}

pub(crate) fn grounding_explanation_search_hits(
    snapshot: &GroundingSnapshotDto,
    limit: usize,
) -> Vec<SearchHit> {
    let mut candidates = Vec::new();
    let mut order = 0usize;
    for symbol in snapshot
        .files
        .iter()
        .flat_map(|file| file.symbols.iter())
        .chain(snapshot.root_symbols.iter())
    {
        let name = grounding_symbol_name(symbol);
        if name.is_empty() || is_import_like_name(&name) {
            continue;
        }
        candidates.push(RecommendationCandidate {
            path: grounding_symbol_path(symbol),
            symbol,
            name,
            order,
        });
        order = order.saturating_add(1);
    }
    candidates.sort_by(compare_recommendation_candidates);

    let use_primary_candidates = candidates
        .iter()
        .any(|candidate| !low_value_recommendation_candidate(candidate));
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for candidate in candidates {
        if use_primary_candidates && low_value_recommendation_candidate(&candidate) {
            continue;
        }
        let key = normalized_recommendation_key(&candidate.name);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        hits.push(search_hit_from_grounding_recommendation(&candidate));
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

fn search_hit_from_grounding_recommendation(candidate: &RecommendationCandidate<'_>) -> SearchHit {
    let evidence_tier = candidate
        .symbol
        .evidence_tier
        .unwrap_or(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph);
    let resolution_status = candidate
        .symbol
        .resolution_status
        .unwrap_or(codestory_contracts::api::PacketEvidenceResolutionDto::Resolved);
    let mut hit = SearchHit {
        node_id: candidate.symbol.id.clone(),
        display_name: candidate.name.clone(),
        kind: candidate.symbol.kind,
        file_path: candidate.path.clone(),
        line: candidate.symbol.line,
        score: 1.0,
        origin: SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: Some(evidence_tier),
        evidence_producer: candidate
            .symbol
            .evidence_producer
            .clone()
            .or_else(|| Some("grounding_recommendation".to_string())),
        resolution_status: Some(resolution_status),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: Some(RetrievalScoreBreakdownDto {
            lexical: 0.45,
            semantic: 0.0,
            graph: 0.55,
            total: 1.0,
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
            provenance: Vec::new(),
        }),
    };
    decorate_search_hit_evidence(&mut hit);
    hit
}

impl AppController {
    pub fn grounding_snapshot(
        &self,
        budget: GroundingBudgetDto,
    ) -> Result<GroundingSnapshotDto, ApiError> {
        self.ensure_consistent_read_state("Grounding")?;
        let root = self.require_project_root()?;
        let storage = self.open_storage_read_only()?;
        if matches!(budget, GroundingBudgetDto::Max)
            && !storage.snapshots().has_ready_detail().map_err(|e| {
                ApiError::internal(format!(
                    "Failed to query grounding detail snapshot readiness: {e}"
                ))
            })?
        {
            return Err(ApiError::new(
                "cache_busy",
                "The published index does not have a finalized grounding detail snapshot. Refresh the index before requesting max grounding.",
            ));
        }
        let config = budget_config(budget);

        let stats = storage
            .get_stats()
            .map_err(|e| ApiError::internal(format!("Failed to query stats: {e}")))?;
        let file_summaries = storage.get_grounding_file_summaries().map_err(|e| {
            ApiError::internal(format!("Failed to load grounding file summaries: {e}"))
        })?;
        let file_languages = file_summaries
            .iter()
            .map(|summary| (summary.file.id, summary.file.language.clone()))
            .collect::<HashMap<_, _>>();
        let file_roles = file_summaries
            .iter()
            .filter_map(|summary| summary.file_role.map(|role| (summary.file.id, role)))
            .collect::<HashMap<_, _>>();
        let mut architecture_root_files = file_summaries
            .iter()
            .filter_map(|summary| {
                let relative = relative_path(&root, &summary.file.path);
                let path_rank = architecture_path_rank(Some(&relative));
                let role = file_roles.get(&summary.file.id).copied();
                (is_production_file_role(role)
                    && (role == Some(FileRole::Entrypoint) || path_rank <= 1))
                    .then_some((
                        grounding_root_file_role_rank(role),
                        path_rank,
                        summary.best_node_rank,
                        summary.symbol_count,
                        relative,
                        summary.file.id,
                    ))
            })
            .collect::<Vec<_>>();
        architecture_root_files.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(left.2.cmp(&right.2))
                .then(right.3.cmp(&left.3))
                .then(left.4.cmp(&right.4))
                .then(left.5.cmp(&right.5))
        });
        architecture_root_files.truncate(ARCHITECTURE_ROOT_FILE_LIMIT);
        let architecture_root_file_ids = architecture_root_files
            .into_iter()
            .map(|candidate| candidate.5)
            .collect::<Vec<_>>();
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
            fatal_error_count: clamp_i64_to_u32(stats.fatal_error_count),
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
        // Every budget truncates the same bounded universe so stricter output
        // remains an exact prefix as role and name diversity are applied.
        let max_root_symbols = budget_config(GroundingBudgetDto::Max).root_symbols;
        let total_root_candidates = storage
            .get_grounding_root_symbol_candidate_count()
            .map_err(|e| {
                ApiError::internal(format!("Failed to count grounding root symbols: {e}"))
            })?;
        let root_fetch_limit = max_root_symbols
            .saturating_mul(ROOT_CANDIDATE_MULTIPLIER)
            .max(max_root_symbols);
        let architecture_exact_names = ARCHITECTURE_ROOT_EXACT_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        let architecture_uppercase_globs = ARCHITECTURE_ROOT_UPPERCASE_GLOBS
            .iter()
            .map(|glob| (*glob).to_string())
            .collect::<Vec<_>>();
        let mut root_records = storage
            .get_grounding_named_root_symbols_for_files(
                &architecture_root_file_ids,
                &architecture_exact_names,
                &architecture_uppercase_globs,
                ARCHITECTURE_NAMED_ROOTS_PER_FILE,
            )
            .map_err(|e| {
                ApiError::internal(format!(
                    "Failed to load named architecture grounding roots: {e}"
                ))
            })?;
        root_records.extend(
            storage
                .get_grounding_root_symbols_for_files(
                    &architecture_root_file_ids,
                    ARCHITECTURE_ROOT_SYMBOL_SCAN_LIMIT,
                )
                .map_err(|e| {
                    ApiError::internal(format!(
                        "Failed to load architecture grounding root symbols: {e}"
                    ))
                })?,
        );
        root_records.extend(
            storage
                .get_grounding_root_symbol_candidates(root_fetch_limit, 0)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to load grounding root symbols: {e}"))
                })?,
        );
        root_records = dedupe_grounding_node_records(root_records);
        let evaluated_root_records = root_records.clone();
        let candidate_node_ids = root_records
            .iter()
            .map(|record| record.node.id)
            .collect::<Vec<_>>();
        let candidate_edge_degrees = build_grounding_edge_degree_map(
            storage
                .get_grounding_edge_digest_counts(&candidate_node_ids)
                .map_err(|e| {
                    ApiError::internal(format!("Failed to load grounding root graph evidence: {e}"))
                })?,
        );
        let candidate_member_counts = storage
            .get_grounding_member_counts(&candidate_node_ids)
            .map_err(|e| {
                ApiError::internal(format!(
                    "Failed to load grounding root member evidence: {e}"
                ))
            })?;
        root_records = diversify_grounding_root_records(
            root_records,
            &root,
            &file_roles,
            &file_languages,
            &candidate_edge_degrees,
            &candidate_member_counts,
        );
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
        let structural_units = storage
            .get_structural_text_units_for_nodes(&displayed_node_ids)
            .map_err(|e| {
                ApiError::internal(format!(
                    "Failed to load structural grounding provenance: {e}"
                ))
            })?
            .into_iter()
            .map(|unit| (unit.node_id, unit))
            .collect::<HashMap<_, _>>();
        let symbol_digest_context = SymbolDigestContext {
            member_counts: &member_counts,
            fallback_lines: &fallback_lines,
            edge_digests: &edge_digests,
            summaries: &summaries,
            structural_units: &structural_units,
        };

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
                        &symbol_digest_context,
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
        let orientation = grounding_orientation(
            &root,
            &evaluated_root_records,
            &root_records,
            total_root_candidates,
            compressed_files,
            &file_roles,
            &file_languages,
        );

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
                &symbol_digest_context,
            ));
        }

        let recommended_queries = recommended_grounding_queries(&root_symbols, &file_digests);

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
        let retrieval =
            retrieval_state_from_storage_for_runtime(&storage, &self.runtime_config).ok();
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
                    codestory_contracts::api::RetrievalFallbackReasonDto::DegradedRuntime => {
                        "degraded_runtime"
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
            orientation,
            root_symbols,
            files: file_digests,
            coverage_buckets,
            notes,
            recommended_queries,
        })
    }

    pub fn symbol_context(&self, node_id: NodeId) -> Result<SymbolContextDto, ApiError> {
        let storage = self.open_storage_read_only()?;
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
            .resolve_indexed_symbol_candidates(&node.display_name, 6)?
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
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "Symbol has no source file; use symbol/trail children or occurrences to choose a path-backed anchor.",
                )
            })?;
        let line = node
            .start_line
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "Symbol has no source line; use occurrences or a child method before requesting a snippet.",
                )
            })?;
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
            scope: codestory_contracts::api::SnippetScopeDto::LineContext,
            requested_context: context_lines as u32,
            snippet_truncated: bounded.truncated,
            max_snippet_bytes: Some(crate::DIRECT_SNIPPET_MAX_BYTES as u32),
            range_source: None,
            fallback_reason: None,
            truncation_guidance: snippet_truncation_guidance(bounded.truncated, context_lines),
        })
    }

    pub fn snippet_function_body_context(
        &self,
        node_id: NodeId,
        context_lines: usize,
    ) -> Result<SnippetContextDto, ApiError> {
        let node = self.node_details(NodeDetailsRequest { id: node_id })?;
        let path = node
            .file_path
            .clone()
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "Symbol has no source file; use symbol/trail children or occurrences to choose a path-backed anchor.",
                )
            })?;
        let line = node
            .start_line
            .ok_or_else(|| {
                ApiError::invalid_argument(
                    "Symbol has no source line; use occurrences or a child method before requesting a snippet.",
                )
            })?;
        let range = match node.end_line.filter(|end| *end >= line) {
            Some(end_line) if end_line > line => Some(FunctionBodyRange {
                end_line,
                range_source: "indexed_symbol_range",
                fallback_reason: None,
            }),
            Some(end_line) => {
                match self.brace_balanced_function_body_end_line(&path, line)? {
                    Some(fallback_end_line) if fallback_end_line > end_line => {
                        Some(FunctionBodyRange {
                            end_line: fallback_end_line,
                            range_source: "brace_balanced_fallback",
                            fallback_reason: Some(format!(
                                "indexed function-body range ended at line {end_line}; expanded with bounded brace-balanced fallback"
                            )),
                        })
                    }
                    _ => Some(FunctionBodyRange {
                        end_line,
                        range_source: "indexed_symbol_range",
                        fallback_reason: None,
                    }),
                }
            }
            None => self
                .brace_balanced_function_body_end_line(&path, line)?
                .map(|end_line| FunctionBodyRange {
                    end_line,
                    range_source: "brace_balanced_fallback",
                    fallback_reason: Some(
                        "indexed function-body range was unavailable; inferred a bounded brace-balanced range"
                            .to_string(),
                    ),
                }),
        };
        let Some(range) = range else {
            let mut context = self.snippet_context(node.id.clone(), context_lines)?;
            context.fallback_reason = Some(
                "function-body range was unavailable and brace-balanced fallback did not find a supported body; fell back to line_context"
                    .to_string(),
            );
            context.range_source = Some("line_context".to_string());
            return Ok(context);
        };
        let (path, bounded) = self.bounded_file_snippet_range(
            &path,
            crate::BoundedSnippetRangeOptions {
                focus_line: line,
                start_line: line,
                end_line: range.end_line,
                context_lines,
                max_bytes: crate::DIRECT_SNIPPET_MAX_BYTES,
                truncation_suffix: crate::DIRECT_SNIPPET_TRUNCATION_SUFFIX,
            },
        )?;

        Ok(SnippetContextDto {
            node,
            path,
            line,
            snippet: bounded.markdown,
            scope: codestory_contracts::api::SnippetScopeDto::FunctionBody,
            requested_context: context_lines as u32,
            snippet_truncated: bounded.truncated,
            max_snippet_bytes: Some(crate::DIRECT_SNIPPET_MAX_BYTES as u32),
            range_source: Some(range.range_source.to_string()),
            fallback_reason: range.fallback_reason,
            truncation_guidance: snippet_truncation_guidance(bounded.truncated, context_lines),
        })
    }

    fn brace_balanced_function_body_end_line(
        &self,
        path: &str,
        start_line: u32,
    ) -> Result<Option<u32>, ApiError> {
        if !path_supports_brace_balanced_function_fallback(path) {
            return Ok(None);
        }
        let source = self.read_file_text(codestory_contracts::api::ReadFileTextRequest {
            path: path.to_string(),
        })?;
        Ok(brace_balanced_body_end_line(&source.text, start_line))
    }
}

struct FunctionBodyRange {
    end_line: u32,
    range_source: &'static str,
    fallback_reason: Option<String>,
}

fn snippet_truncation_guidance(truncated: bool, context_lines: usize) -> Option<String> {
    truncated.then(|| {
        format!(
            "rerun with a smaller --context than {context_lines}, choose a narrower symbol, or use source verification for the omitted tail"
        )
    })
}

fn path_supports_brace_balanced_function_fallback(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ts" | "tsx"
                    | "js"
                    | "jsx"
                    | "rs"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "cxx"
                    | "h"
                    | "hh"
                    | "hpp"
                    | "hxx"
                    | "java"
                    | "cs"
                    | "go"
            )
        })
        .unwrap_or(false)
}

fn brace_balanced_body_end_line(source: &str, start_line: u32) -> Option<u32> {
    let start_index = start_line.checked_sub(1)? as usize;
    let mut depth = 0usize;
    let mut saw_opening_brace = false;
    let mut in_block_comment = false;

    for (offset, line) in source
        .lines()
        .enumerate()
        .skip(start_index)
        .take(FUNCTION_BODY_FALLBACK_MAX_SCAN_LINES)
    {
        scan_braces_in_line(
            line,
            &mut depth,
            &mut saw_opening_brace,
            &mut in_block_comment,
        );
        if saw_opening_brace && depth == 0 {
            return Some((offset + 1) as u32);
        }
        if !saw_opening_brace
            && offset.saturating_sub(start_index) >= FUNCTION_BODY_FALLBACK_BRACE_SEARCH_LINES
        {
            return None;
        }
    }
    None
}

fn scan_braces_in_line(
    line: &str,
    depth: &mut usize,
    saw_opening_brace: &mut bool,
    in_block_comment: &mut bool,
) {
    let mut chars = line.chars().peekable();
    let mut string_delimiter: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if *in_block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                *in_block_comment = false;
            }
            continue;
        }
        if let Some(delimiter) = string_delimiter {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == delimiter {
                string_delimiter = None;
            }
            continue;
        }
        if ch == '/' {
            match chars.peek() {
                Some('/') => break,
                Some('*') => {
                    chars.next();
                    *in_block_comment = true;
                    continue;
                }
                _ => {}
            }
        }
        if matches!(ch, '"' | '\'' | '`') {
            string_delimiter = Some(ch);
            continue;
        }
        if ch == '{' {
            *saw_opening_brace = true;
            *depth = depth.saturating_add(1);
            continue;
        }
        if ch == '}' && *saw_opening_brace {
            *depth = depth.saturating_sub(1);
        }
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
        insert_file_node_with_role(storage, file_id, path, FileRole::Source, child)
    }

    fn insert_file_node_with_role(
        storage: &mut Storage,
        file_id: i64,
        path: &Path,
        file_role: FileRole,
        child: Node,
    ) -> Result<(), Box<dyn std::error::Error>> {
        insert_file_node_with_role_and_language(storage, file_id, path, file_role, "rust", child)
    }

    fn insert_file_node_with_role_and_language(
        storage: &mut Storage,
        file_id: i64,
        path: &Path,
        file_role: FileRole,
        language: &str,
        child: Node,
    ) -> Result<(), Box<dyn std::error::Error>> {
        storage.insert_file(&FileInfo {
            id: file_id,
            path: path.to_path_buf(),
            language: language.to_string(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 10,
            file_role,
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
    fn grounding_root_diversity_does_not_consume_duplicate_name_subsystems() {
        let root = Path::new("/repo");
        let record = |node_id, file_id, name: &str, path: &str| GroundingNodeRecord {
            node: Node {
                id: CoreNodeId(node_id),
                kind: NodeKind::STRUCT,
                serialized_name: name.to_string(),
                file_node_id: Some(CoreNodeId(file_id)),
                start_line: Some(1),
                ..Default::default()
            },
            display_name: name.to_string(),
            file_path: Some(root.join(path)),
        };
        let records = vec![
            record(101, 10, "SharedRoot", "packages/a/src/lib.ts"),
            record(201, 20, "SharedRoot", "packages/b/src/lib.ts"),
            record(202, 20, "UniqueB", "packages/b/src/lib.ts"),
            record(301, 30, "UniqueC", "packages/c/src/lib.ts"),
        ];
        let file_roles = [
            (10, FileRole::Source),
            (20, FileRole::Source),
            (30, FileRole::Source),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();
        let file_languages = [
            (10, "typescript".to_string()),
            (20, "typescript".to_string()),
            (30, "typescript".to_string()),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        assert_ne!(
            grounding_root_subsystem_key(root, &records[0], &file_languages),
            grounding_root_subsystem_key(root, &records[1], &file_languages)
        );
        let diversified = diversify_grounding_root_records(
            records,
            root,
            &file_roles,
            &file_languages,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            diversified
                .iter()
                .take(3)
                .map(|record| (
                    grounding_root_terminal_name(record),
                    grounding_root_subsystem_key(root, record, &file_languages)
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    "sharedroot".to_string(),
                    "typescript:packages/a/src".to_string()
                ),
                (
                    "uniqueb".to_string(),
                    "typescript:packages/b/src".to_string()
                ),
                (
                    "uniquec".to_string(),
                    "typescript:packages/c/src".to_string()
                ),
            ]
        );
    }

    #[test]
    fn grounding_entrypoint_evidence_requires_production_callable_name_evidence() {
        let root = Path::new("/repo");
        let record = |name: &str| GroundingNodeRecord {
            node: Node {
                id: CoreNodeId(101),
                kind: NodeKind::FUNCTION,
                serialized_name: name.to_string(),
                file_node_id: Some(CoreNodeId(10)),
                start_line: Some(1),
                ..Default::default()
            },
            display_name: name.to_string(),
            file_path: Some(root.join("src/main.ts")),
        };
        let mut roles = [(10, FileRole::Entrypoint)]
            .into_iter()
            .collect::<HashMap<_, _>>();

        assert!(is_grounding_entrypoint_root(
            root,
            &record("startApplication"),
            &roles
        ));
        assert!(!is_grounding_entrypoint_root(
            root,
            &record("helper"),
            &roles
        ));
        assert!(!is_grounding_entrypoint_root(
            root,
            &record("startupCache"),
            &roles
        ));
        assert!(is_grounding_entrypoint_root(
            root,
            &record("ComicPage"),
            &roles
        ));
        assert!(!is_grounding_entrypoint_root(
            root,
            &record("isHomepage"),
            &roles
        ));

        roles.insert(10, FileRole::Test);
        assert!(!is_grounding_entrypoint_root(root, &record("main"), &roles));
    }

    fn grounding_symbol(
        id: &str,
        label: &str,
        kind: codestory_contracts::api::NodeKind,
        member_count: Option<u32>,
    ) -> GroundingSymbolDigestDto {
        GroundingSymbolDigestDto {
            id: codestory_contracts::api::NodeId(id.to_string()),
            node_ref: label
                .split_once(" @ ")
                .map(|(_, path)| format!("{path}:1:{}", label.split(" @ ").next().unwrap())),
            label: label.to_string(),
            kind,
            line: Some(1),
            member_count,
            summary: None,
            edge_digest: Vec::new(),
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
        }
    }

    #[test]
    fn recommended_grounding_queries_prefer_architecture_anchors() {
        let fixture_symbols = vec![
            grounding_symbol(
                "fixture-js",
                "Notifier @ crates/codestory-indexer/tests/fixtures/javascript_workflow.js",
                codestory_contracts::api::NodeKind::CLASS,
                Some(1),
            ),
            grounding_symbol(
                "fixture-rs",
                "Notifier @ crates/codestory-indexer/tests/fixtures/rust_workflow.rs",
                codestory_contracts::api::NodeKind::INTERFACE,
                Some(1),
            ),
            grounding_symbol(
                "javaparser",
                "com.github.javaparser.ast.visitor.CloneVisitor @ bin/app/user/projects/javaparser/src/main/java/com/github/javaparser/ast/visitor/CloneVisitor.java",
                codestory_contracts::api::NodeKind::CLASS,
                Some(2),
            ),
            grounding_symbol(
                "sqlite",
                "sqlite3 @ src/external/sqlite/sqlite3.c",
                codestory_contracts::api::NodeKind::CLASS,
                Some(2),
            ),
            grounding_symbol(
                "testing-fixture",
                "Bar @ testing/project_setup/custom_command_python/data/src/bar.py",
                codestory_contracts::api::NodeKind::CLASS,
                Some(2),
            ),
        ];
        let files = vec![
            GroundingFileDigestDto {
                file_path: "crates/codestory-runtime/src/lib.rs".to_string(),
                language: Some("rust".to_string()),
                symbol_count: 4,
                represented_symbol_count: 4,
                compressed: true,
                symbols: vec![
                    grounding_symbol(
                        "storage",
                        "Storage @ crates/codestory-runtime/src/lib.rs",
                        codestory_contracts::api::NodeKind::TYPEDEF,
                        None,
                    ),
                    grounding_symbol(
                        "runtime",
                        "Runtime @ crates/codestory-runtime/src/lib.rs",
                        codestory_contracts::api::NodeKind::STRUCT,
                        Some(12),
                    ),
                ],
            },
            GroundingFileDigestDto {
                file_path: "crates/codestory-indexer/src/lib.rs".to_string(),
                language: Some("rust".to_string()),
                symbol_count: 2,
                represented_symbol_count: 2,
                compressed: true,
                symbols: vec![
                    grounding_symbol(
                        "language-config",
                        "LanguageConfig @ crates/codestory-indexer/src/lib.rs",
                        codestory_contracts::api::NodeKind::STRUCT,
                        Some(6),
                    ),
                    grounding_symbol(
                        "std-string",
                        "std::wstring @ crates/codestory-indexer/src/lib.rs",
                        codestory_contracts::api::NodeKind::CLASS,
                        Some(1),
                    ),
                ],
            },
        ];

        let recommended = recommended_grounding_queries(&fixture_symbols, &files);

        assert_eq!(recommended.first().map(String::as_str), Some("Runtime"));
        assert!(recommended.iter().any(|query| query == "LanguageConfig"));
        assert!(!recommended.iter().any(|query| query == "Notifier"));
        assert!(!recommended.iter().any(|query| query.contains("javaparser")));
        assert!(!recommended.iter().any(|query| query == "sqlite3"));
        assert!(!recommended.iter().any(|query| query == "Bar"));
        assert!(!recommended.iter().any(|query| query == "std::wstring"));
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
    fn grounding_snapshot_publishes_structural_text_metadata_without_graph_claims() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        let manifest = temp.path().join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"demo\"\n").expect("write manifest");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let projected = codestory_indexer::structural::index_structural_file(&manifest)
                .expect("collect manifest");
            storage
                .projections()
                .flush_projection_batch(codestory_store::ProjectionBatch {
                    files: &projected.files,
                    file_content_hashes: &projected.file_content_hashes,
                    nodes: &projected.nodes,
                    structural_text_units: &projected.structural_text_units,
                    structural_text_projections: &projected.structural_text_projections,
                    structural_text_cache_writes: &[],
                    edges: &projected.edges,
                    occurrences: &projected.occurrences,
                    component_access: &projected.component_access,
                    callable_projection_states: &projected.callable_projection_states,
                    file_errors: &[],
                })
                .expect("insert verified manifest projection");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");
        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("grounding snapshot");
        let symbol = snapshot
            .files
            .iter()
            .flat_map(|file| file.symbols.iter())
            .find(|symbol| symbol.label.starts_with("demo"))
            .expect("manifest symbol");

        assert_eq!(
            symbol.evidence_tier,
            Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
        );
        assert_eq!(
            symbol.evidence_producer.as_deref(),
            Some("structural_cargo_manifest_collector")
        );
        assert_eq!(
            symbol.resolution_status,
            Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
        );

        let hit = grounding_explanation_search_hits(&snapshot, 8)
            .into_iter()
            .find(|hit| hit.node_id == symbol.id)
            .expect("grounding explanation hit");
        assert_eq!(
            hit.evidence_tier,
            Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
        );
        assert_eq!(
            hit.resolution_status,
            Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn grounding_snapshot_preserves_openapi_endpoint_source_identity() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        let schema = temp.path().join("openapi.json");
        std::fs::write(&schema, "{\"openapi\":\"3.0.0\"}\n").expect("write schema");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            insert_file_node(
                &mut storage,
                12,
                &schema,
                Node {
                    id: CoreNodeId(102),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "GET /api/users".to_string(),
                    canonical_id: Some("openapi:endpoint:GET /api/users".to_string()),
                    file_node_id: Some(CoreNodeId(12)),
                    start_line: Some(1),
                    ..Default::default()
                },
            )
            .expect("insert OpenAPI endpoint node");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");
        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("grounding snapshot");
        let symbol = snapshot
            .files
            .iter()
            .flat_map(|file| file.symbols.iter())
            .find(|symbol| symbol.label.starts_with("GET /api/users"))
            .expect("OpenAPI endpoint symbol");

        assert_eq!(
            symbol.evidence_tier,
            Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
        );
        assert_eq!(
            symbol.evidence_producer.as_deref(),
            Some("openapi_endpoint_schema")
        );
        assert_eq!(
            symbol.resolution_status,
            Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
        );

        let candidate = RecommendationCandidate {
            symbol,
            name: "GET /api/users".to_string(),
            path: Some("openapi.json".to_string()),
            order: 0,
        };
        let hit = search_hit_from_grounding_recommendation(&candidate);
        assert_eq!(
            hit.evidence_tier,
            Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("openapi_endpoint_schema")
        );
        assert_eq!(
            hit.resolution_status,
            Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
    }

    #[test]
    fn function_body_snippet_uses_symbol_range_when_available() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        let source_path = temp.path().join("src").join("lib.rs");
        std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        std::fs::write(
            &source_path,
            "fn before() {}\n\nfn route_handler() {\n    let first = 1;\n    let decisive = payload.create();\n    let last = first;\n}\n\nfn after() {}\n",
        )
        .expect("write source");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            insert_file_node(
                &mut storage,
                11,
                &source_path,
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "route_handler".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(3),
                    end_line: Some(7),
                    ..Default::default()
                },
            )
            .expect("insert function");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snippet = controller
            .snippet_function_body_context(codestory_contracts::api::NodeId("101".to_string()), 0)
            .expect("function body snippet");

        assert_eq!(
            snippet.scope,
            codestory_contracts::api::SnippetScopeDto::FunctionBody
        );
        assert!(snippet.snippet.contains("fn route_handler()"));
        assert!(snippet.snippet.contains("payload.create()"));
        assert!(!snippet.snippet.contains("fn before()"));
        assert!(!snippet.snippet.contains("fn after()"));
    }

    #[test]
    fn function_body_snippet_uses_brace_balanced_fallback_when_range_is_missing() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        let source_path = temp
            .path()
            .join("src")
            .join("app")
            .join("api")
            .join("comments")
            .join("route.ts");
        std::fs::create_dir_all(source_path.parent().expect("route parent"))
            .expect("create route parent");
        std::fs::write(
            &source_path,
            "import { getPayload } from 'payload';\n\nexport async function POST(request: Request) {\n    const payload = await getPayload();\n    const existing = await payload.find({ collection: 'comments' });\n    if (!existing.totalDocs) {\n        const saved = await payload.create({ collection: 'comments', data: { body: 'ok' } });\n        return Response.json(saved);\n    }\n    return Response.json({ ok: true });\n}\n\nexport function helper() { return null; }\n",
        )
        .expect("write route");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            insert_file_node(
                &mut storage,
                11,
                &source_path,
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "POST".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(3),
                    end_line: None,
                    ..Default::default()
                },
            )
            .expect("insert function");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snippet = controller
            .snippet_function_body_context(codestory_contracts::api::NodeId("101".to_string()), 0)
            .expect("function body snippet");

        assert_eq!(
            snippet.scope,
            codestory_contracts::api::SnippetScopeDto::FunctionBody
        );
        assert_eq!(
            snippet.range_source.as_deref(),
            Some("brace_balanced_fallback")
        );
        assert!(
            snippet.fallback_reason.as_deref().is_some_and(
                |reason| reason.contains("indexed function-body range was unavailable")
            )
        );
        assert!(snippet.snippet.contains("payload.find"));
        assert!(snippet.snippet.contains("payload.create"));
        assert!(!snippet.snippet.contains("helper"));
    }

    #[test]
    fn function_body_snippet_reports_line_context_fallback_when_body_is_unavailable() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        let source_path = temp.path().join("src").join("route.ts");
        std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
        std::fs::write(
            &source_path,
            "export const POST = makeHandler(payload.create);\n",
        )
        .expect("write route");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            insert_file_node(
                &mut storage,
                11,
                &source_path,
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "POST".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    end_line: None,
                    ..Default::default()
                },
            )
            .expect("insert function");
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");

        let snippet = controller
            .snippet_function_body_context(codestory_contracts::api::NodeId("101".to_string()), 0)
            .expect("line context fallback");

        assert_eq!(
            snippet.scope,
            codestory_contracts::api::SnippetScopeDto::LineContext
        );
        assert_eq!(snippet.range_source.as_deref(), Some("line_context"));
        assert!(
            snippet
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("fell back to line_context"))
        );
        assert!(snippet.snippet.contains("payload.create"));
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
                    file_role: FileRole::Source,
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
    fn grounding_snapshot_prefers_production_and_diversifies_fixture_roots() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            for fixture_index in 0..10_i64 {
                let path = temp
                    .path()
                    .join("crates/codestory-indexer/tests/fixtures")
                    .join(format!("fixture_{fixture_index}.rs"));
                std::fs::create_dir_all(path.parent().expect("fixture parent"))
                    .expect("create fixture parent");
                std::fs::write(&path, "struct FixtureRoot;\n").expect("write fixture");
                let name = if fixture_index < 7 {
                    "Notifier".to_string()
                } else {
                    format!("FixtureRoot{fixture_index}")
                };
                let file_role = match fixture_index {
                    7 => FileRole::Vendor,
                    8 => FileRole::Generated,
                    9 => FileRole::Benchmark,
                    _ => FileRole::Test,
                };
                insert_file_node_with_role(
                    &mut storage,
                    100 + fixture_index,
                    &path,
                    file_role,
                    Node {
                        id: CoreNodeId(1_000 + fixture_index),
                        kind: NodeKind::STRUCT,
                        serialized_name: name,
                        file_node_id: Some(CoreNodeId(100 + fixture_index)),
                        start_line: Some(1 + fixture_index as u32),
                        ..Default::default()
                    },
                )
                .expect("insert fixture root");
            }
            for source_index in 0..20_i64 {
                let path = temp
                    .path()
                    .join("crates/codestory-runtime/src")
                    .join(format!("production_{source_index}.rs"));
                std::fs::create_dir_all(path.parent().expect("source parent"))
                    .expect("create source parent");
                std::fs::write(&path, "struct ProductionRoot;\n").expect("write source");
                insert_file_node_with_role(
                    &mut storage,
                    200 + source_index,
                    &path,
                    if source_index == 0 {
                        FileRole::Entrypoint
                    } else {
                        FileRole::Source
                    },
                    Node {
                        id: CoreNodeId(2_000 + source_index),
                        kind: NodeKind::STRUCT,
                        serialized_name: format!("ProductionRoot{source_index}"),
                        file_node_id: Some(CoreNodeId(200 + source_index)),
                        start_line: Some(11 + source_index as u32),
                        ..Default::default()
                    },
                )
                .expect("insert production root");
            }

            let node_only_path = temp.path().join("src/compatibility_only.rs");
            storage
                .insert_nodes_batch(&[
                    Node {
                        id: CoreNodeId(400),
                        kind: NodeKind::FILE,
                        serialized_name: node_only_path.to_string_lossy().to_string(),
                        ..Default::default()
                    },
                    Node {
                        id: CoreNodeId(4_000),
                        kind: NodeKind::STRUCT,
                        serialized_name: "CompatibilityRoot".to_string(),
                        file_node_id: Some(CoreNodeId(400)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                ])
                .expect("insert node-only compatibility root");
            assert!(
                storage
                    .get_file_roles_by_paths(&[node_only_path.to_string_lossy().to_string()])
                    .expect("load node-only role")
                    .is_empty()
            );

            let production_file_ids = (200..220).collect::<Vec<_>>();
            let architecture_candidates = storage
                .get_grounding_root_symbols_for_files(&production_file_ids, 16)
                .expect("load architecture root candidates");
            assert_eq!(architecture_candidates.len(), 20);
            assert!(
                architecture_candidates.iter().all(|record| {
                    record.file_path.as_deref().is_some_and(|path| {
                        path.to_string_lossy()
                            .replace('\\', "/")
                            .contains("crates/codestory-runtime/src/production_")
                    })
                }),
                "the bounded architecture window should retain verified production roots"
            );

            storage
                .snapshots()
                .refresh_summary()
                .expect("refresh summary snapshot");
            storage
                .snapshots()
                .refresh_detail()
                .expect("refresh detail snapshot");
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
        let balanced_again = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("repeat balanced snapshot");

        assert_eq!(strict.root_symbols.len(), 8);
        assert_eq!(balanced.root_symbols.len(), 16);
        assert_eq!(max.root_symbols.len(), 28);
        assert!(
            balanced.root_symbols.iter().all(|symbol| {
                symbol
                    .label
                    .contains("crates/codestory-runtime/src/production_")
            }),
            "{:?}",
            balanced
                .root_symbols
                .iter()
                .map(|symbol| &symbol.label)
                .collect::<Vec<_>>()
        );
        assert!(max.root_symbols.iter().take(20).all(|symbol| {
            symbol
                .label
                .contains("crates/codestory-runtime/src/production_")
        }));
        assert!(
            max.root_symbols
                .iter()
                .position(|symbol| symbol.label.starts_with("CompatibilityRoot @ "))
                .is_some_and(|index| index >= 20)
        );
        assert_eq!(
            strict
                .root_symbols
                .iter()
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>(),
            balanced
                .root_symbols
                .iter()
                .take(strict.root_symbols.len())
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            balanced
                .root_symbols
                .iter()
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>(),
            max.root_symbols
                .iter()
                .take(balanced.root_symbols.len())
                .map(|symbol| &symbol.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            balanced
                .root_symbols
                .iter()
                .map(|symbol| (&symbol.id, &symbol.label))
                .collect::<Vec<_>>(),
            balanced_again
                .root_symbols
                .iter()
                .map(|symbol| (&symbol.id, &symbol.label))
                .collect::<Vec<_>>()
        );

        let notifier_positions = max
            .root_symbols
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| {
                symbol
                    .label
                    .split_once(" @ ")
                    .is_some_and(|(name, _)| name == "Notifier")
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let first_notifier = *notifier_positions.first().expect("retained notifier root");
        assert!(first_notifier >= 20);
        assert!(
            notifier_positions
                .iter()
                .skip(1)
                .all(|index| *index >= first_notifier + 4)
        );
    }

    #[test]
    fn grounding_snapshot_ranks_cross_language_architecture_and_reports_orientation() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            let frontend = temp.path().join("src/main.ts");
            insert_file_node_with_role_and_language(
                &mut storage,
                101,
                &frontend,
                FileRole::Entrypoint,
                "typescript",
                Node {
                    id: CoreNodeId(1_001),
                    kind: NodeKind::INTERFACE,
                    serialized_name: "AppConfig".to_string(),
                    file_node_id: Some(CoreNodeId(101)),
                    start_line: Some(1),
                    ..Default::default()
                },
            )
            .expect("insert frontend entrypoint");
            let mut frontend_nodes = vec![Node {
                id: CoreNodeId(1_002),
                kind: NodeKind::FUNCTION,
                serialized_name: "startApplication".to_string(),
                file_node_id: Some(CoreNodeId(101)),
                start_line: Some(100),
                ..Default::default()
            }];
            for offset in 0..20_i64 {
                frontend_nodes.push(Node {
                    id: CoreNodeId(1_200 + offset),
                    kind: NodeKind::INTERFACE,
                    serialized_name: format!("LeafType{offset}"),
                    file_node_id: Some(CoreNodeId(101)),
                    start_line: Some(2 + offset as u32),
                    ..Default::default()
                });
            }
            frontend_nodes.push(Node {
                id: CoreNodeId(1_003),
                kind: NodeKind::FUNCTION,
                serialized_name: "helper".to_string(),
                file_node_id: Some(CoreNodeId(101)),
                start_line: Some(30),
                ..Default::default()
            });
            let mut frontend_edges = Vec::new();
            for offset in 0..4_i64 {
                frontend_nodes.push(Node {
                    id: CoreNodeId(1_100 + offset),
                    kind: NodeKind::VARIABLE,
                    serialized_name: format!("runtimeDependency{offset}"),
                    file_node_id: Some(CoreNodeId(101)),
                    start_line: Some(101 + offset as u32),
                    ..Default::default()
                });
                frontend_edges.push(Edge {
                    id: EdgeId(1_500 + offset),
                    source: CoreNodeId(1_002),
                    target: CoreNodeId(1_100 + offset),
                    kind: EdgeKind::MEMBER,
                    file_node_id: Some(CoreNodeId(101)),
                    line: Some(101 + offset as u32),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                });
            }
            storage
                .insert_nodes_batch(&frontend_nodes)
                .expect("insert frontend graph");
            storage
                .insert_edges_batch(&frontend_edges)
                .expect("insert frontend graph evidence");

            for (file_id, node_id, path, language, name) in [
                (
                    201,
                    2_001,
                    "crates/engine/src/lib.rs",
                    "rust",
                    "EngineRuntime",
                ),
                (
                    301,
                    3_001,
                    "src/core/service.ts",
                    "typescript",
                    "DomainService",
                ),
                (401, 4_001, "src/db/store.ts", "typescript", "ProjectStore"),
            ] {
                insert_file_node_with_role_and_language(
                    &mut storage,
                    file_id,
                    &temp.path().join(path),
                    FileRole::classify_path(Path::new(path)),
                    language,
                    Node {
                        id: CoreNodeId(node_id),
                        kind: NodeKind::STRUCT,
                        serialized_name: name.to_string(),
                        file_node_id: Some(CoreNodeId(file_id)),
                        start_line: Some(4),
                        ..Default::default()
                    },
                )
                .expect("insert architecture boundary");
            }

            for index in 0..12_i64 {
                let file_id = 500 + index;
                insert_file_node_with_role_and_language(
                    &mut storage,
                    file_id,
                    &temp.path().join(format!("src/types/leaf_{index}.ts")),
                    FileRole::Source,
                    "typescript",
                    Node {
                        id: CoreNodeId(5_000 + index),
                        kind: NodeKind::TYPEDEF,
                        serialized_name: format!("LeafAlias{index}"),
                        file_node_id: Some(CoreNodeId(file_id)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                )
                .expect("insert leaf alias");
            }

            storage
                .snapshots()
                .refresh_summary()
                .expect("refresh summary snapshot");
            storage
                .snapshots()
                .refresh_detail()
                .expect("refresh detail snapshot");
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
        let balanced_again = controller
            .grounding_snapshot(GroundingBudgetDto::Balanced)
            .expect("repeat balanced snapshot");

        let strict_names = strict
            .root_symbols
            .iter()
            .map(grounding_symbol_name)
            .collect::<Vec<_>>();
        assert_eq!(
            strict_names.first().map(String::as_str),
            Some("startApplication"),
            "graph-connected entrypoint should outrank the leaf declaration in the same file"
        );
        for boundary in ["EngineRuntime", "DomainService", "ProjectStore"] {
            assert!(
                strict_names.iter().any(|name| name == boundary),
                "strict grounding omitted architecture boundary {boundary}: {strict_names:?}"
            );
        }
        let root_refs = |snapshot: &GroundingSnapshotDto| {
            snapshot
                .root_symbols
                .iter()
                .map(|symbol| symbol.node_ref.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            root_refs(&strict),
            root_refs(&balanced)[..strict.root_symbols.len()]
        );
        assert_eq!(
            root_refs(&balanced),
            root_refs(&max)[..balanced.root_symbols.len()]
        );
        assert_eq!(root_refs(&balanced), root_refs(&balanced_again));
        assert_eq!(
            strict.orientation.confidence,
            GroundingOrientationConfidenceDto::Strong,
            "{:?}",
            strict.orientation
        );
        assert_eq!(strict.orientation.selected_entrypoint_roots, 1);
        assert_eq!(strict.orientation.candidate_entrypoint_roots, 1);
        assert!(strict.orientation.selected_subsystems >= 4);
        assert!(
            strict
                .orientation
                .uncertainty
                .contains(&GroundingOrientationUncertaintyDto::CompressedPresentation)
        );
        assert!(
            !strict
                .orientation
                .uncertainty
                .contains(&GroundingOrientationUncertaintyDto::NoEntrypointEvidence)
        );
    }

    #[test]
    fn grounding_snapshot_reports_weak_orientation_without_entrypoint_evidence() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");
        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            for index in 0..3_i64 {
                let file_id = 700 + index;
                insert_file_node_with_role(
                    &mut storage,
                    file_id,
                    &temp.path().join(format!("src/helpers/helper_{index}.rs")),
                    FileRole::Source,
                    Node {
                        id: CoreNodeId(7_000 + index),
                        kind: NodeKind::STRUCT,
                        serialized_name: format!("Helper{index}"),
                        file_node_id: Some(CoreNodeId(file_id)),
                        start_line: Some(1),
                        ..Default::default()
                    },
                )
                .expect("insert helper root");
            }
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");
        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("strict snapshot");

        assert_eq!(
            snapshot.orientation.confidence,
            GroundingOrientationConfidenceDto::Weak
        );
        assert_eq!(snapshot.orientation.candidate_entrypoint_roots, 0);
        assert_eq!(snapshot.orientation.selected_entrypoint_roots, 0);
        assert!(
            snapshot
                .orientation
                .uncertainty
                .contains(&GroundingOrientationUncertaintyDto::NoEntrypointEvidence)
        );
    }

    #[test]
    fn grounding_snapshot_keeps_diversified_fixture_fallback_without_production_roots() {
        let temp = tempdir().expect("temp dir");
        let db_path = temp.path().join("cache").join("codestory.db");
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create db parent");

        {
            let mut storage = Storage::open(&db_path).expect("open storage");
            for fixture_index in 0..12_i64 {
                let path = temp
                    .path()
                    .join("tests/fixtures")
                    .join(format!("fixture_{fixture_index}.rs"));
                std::fs::create_dir_all(path.parent().expect("fixture parent"))
                    .expect("create fixture parent");
                std::fs::write(&path, "struct FixtureRoot;\n").expect("write fixture");
                let name = if fixture_index < 7 {
                    "Notifier".to_string()
                } else {
                    format!("Harness{fixture_index}")
                };
                insert_file_node_with_role(
                    &mut storage,
                    300 + fixture_index,
                    &path,
                    FileRole::Test,
                    Node {
                        id: CoreNodeId(3_000 + fixture_index),
                        kind: NodeKind::STRUCT,
                        serialized_name: name,
                        file_node_id: Some(CoreNodeId(300 + fixture_index)),
                        start_line: Some(1 + fixture_index as u32),
                        ..Default::default()
                    },
                )
                .expect("insert fixture root");
            }
        }

        let controller = AppController::new();
        controller
            .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
            .expect("open project");
        let snapshot = controller
            .grounding_snapshot(GroundingBudgetDto::Strict)
            .expect("strict snapshot");

        assert_eq!(snapshot.root_symbols.len(), 8);
        assert!(
            snapshot
                .root_symbols
                .iter()
                .all(|symbol| symbol.label.contains("tests/fixtures/"))
        );
        assert_eq!(
            snapshot
                .root_symbols
                .iter()
                .take(6)
                .map(|symbol| {
                    symbol
                        .label
                        .split_once(" @ ")
                        .map(|(name, _)| name)
                        .expect("symbol label path")
                })
                .collect::<HashSet<_>>()
                .len(),
            6
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
                        file_role: FileRole::Source,
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
            storage
                .snapshots()
                .refresh_summary()
                .expect("refresh summary snapshot");
            storage
                .snapshots()
                .refresh_detail()
                .expect("refresh detail snapshot");
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
                    file_role: FileRole::Source,
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
                    file_role: FileRole::Source,
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
                    file_role: FileRole::Source,
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
    fn max_grounding_does_not_mutate_an_incomplete_publication() {
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
                    file_role: FileRole::Source,
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
        let error = controller
            .grounding_snapshot(GroundingBudgetDto::Max)
            .expect_err("max snapshot should require a finalized detail publication");
        assert_eq!(error.code, "cache_busy");

        let storage = Storage::open(&db_path).expect("reopen storage");
        assert!(
            !storage
                .snapshots()
                .has_ready_detail()
                .expect("detail snapshot readiness"),
            "a read must not hydrate the live publication"
        );
    }
}
