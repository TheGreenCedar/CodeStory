#[cfg(test)]
use codestory_contracts::api::TrailContextDto;
use codestory_contracts::api::{
    IndexFreshnessDto, NodeId, NodeKind, NodeOccurrencesRequest, RepoTextScanStatsDto,
    RetrievalScoreBreakdownDto, RetrievalShadowDto, SearchHit, SearchMatchQualityDto,
    SearchQueryAssessmentDto, SourceOccurrenceDto,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use crate::{
    args::{
        QueryResolutionOutput, RepoTextMode, SearchHitOutput, SearchOutput,
        VerificationTargetOutput,
    },
    runtime::{self, RuntimeContext},
};

#[derive(Debug, Clone, Copy)]
pub(super) struct RepoTextOutputConfig {
    pub(super) mode: RepoTextMode,
    pub(super) enabled: bool,
}

pub(super) struct SearchOutputParts<'a> {
    pub(super) project_root: &'a std::path::Path,
    pub(super) query: &'a str,
    pub(super) retrieval: &'a codestory_contracts::api::RetrievalStateDto,
    pub(super) retrieval_shadow: Option<&'a RetrievalShadowDto>,
    pub(super) freshness: Option<&'a IndexFreshnessDto>,
    pub(super) symbol_hits: &'a [SearchHit],
    pub(super) repo_text_hits: &'a [SearchHit],
    pub(super) repo_text_stats: Option<&'a RepoTextScanStatsDto>,
    pub(super) query_assessment: Option<&'a SearchQueryAssessmentDto>,
    pub(super) search_plan: Option<&'a codestory_contracts::api::SearchPlanDto>,
    pub(super) suggestions: &'a [SearchHit],
    pub(super) occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    pub(super) limit_per_source: u32,
    pub(super) repo_text: RepoTextOutputConfig,
    pub(super) explain: bool,
}

pub(super) fn build_search_output(parts: SearchOutputParts<'_>) -> SearchOutput {
    let indexed_symbol_hits = parts
        .symbol_hits
        .iter()
        .map(|hit| {
            build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                occurrences_for_hit(parts.occurrences_by_node, hit),
            )
        })
        .collect::<Vec<_>>();
    let mut duplicate_index = HashMap::new();
    for hit in &indexed_symbol_hits {
        if let Some(key) = search_hit_location_key(hit) {
            duplicate_index
                .entry(key)
                .or_insert_with(|| hit.node_id.clone());
        }
    }
    let repo_text_hits = parts
        .repo_text_hits
        .iter()
        .map(|hit| {
            let mut output = build_search_hit_output(
                parts.project_root,
                hit,
                Some(parts.query),
                parts.explain,
                &[],
            );
            if let Some(key) = search_hit_location_key(&output) {
                output.duplicate_of = duplicate_index.get(&key).cloned();
            }
            output
        })
        .collect::<Vec<_>>();
    let query_hints = search_query_hints(parts.query, &indexed_symbol_hits, &repo_text_hits);

    SearchOutput {
        query: parts.query.to_string(),
        retrieval: parts.retrieval.clone(),
        retrieval_shadow: parts.retrieval_shadow.cloned(),
        freshness: parts.freshness.cloned(),
        limit_per_source: parts.limit_per_source,
        repo_text_mode: parts.repo_text.mode,
        repo_text_enabled: parts.repo_text.enabled,
        query_assessment: parts.query_assessment.cloned(),
        search_plan: parts.search_plan.cloned(),
        explain: parts.explain,
        query_hints,
        suggestions: parts
            .suggestions
            .iter()
            .map(|hit| {
                build_search_hit_output(
                    parts.project_root,
                    hit,
                    Some(parts.query),
                    parts.explain,
                    occurrences_for_hit(parts.occurrences_by_node, hit),
                )
            })
            .collect(),
        indexed_symbol_hits,
        repo_text_hits,
        repo_text_stats: parts.repo_text_stats.cloned(),
    }
}

pub(super) fn dedupe_verification_targets(targets: &mut Vec<VerificationTargetOutput>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.role.clone(),
            target.path.clone(),
            target.line,
            target.reason.clone(),
        ))
    });
}

pub(crate) fn build_query_resolution_output(
    project_root: &std::path::Path,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    QueryResolutionOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        resolved: build_search_hit_output(
            project_root,
            &target.selected,
            Some(&target.requested),
            false,
            &[],
        ),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| {
                build_search_hit_output(project_root, hit, Some(&target.requested), false, &[])
            })
            .collect(),
    }
}

pub(super) fn build_query_resolution_output_with_runtime(
    runtime: &RuntimeContext,
    target: &runtime::ResolvedTarget,
) -> QueryResolutionOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        std::iter::once(&target.selected).chain(target.alternatives.iter()),
    );
    build_query_resolution_output_from_occurrences(&runtime.project_root, target, &occurrences)
}

pub(super) fn build_query_resolution_output_from_occurrences(
    project_root: &Path,
    target: &runtime::ResolvedTarget,
    occurrences: &HashMap<NodeId, Vec<SourceOccurrenceDto>>,
) -> QueryResolutionOutput {
    QueryResolutionOutput {
        selector: target.selector,
        requested: target.requested.clone(),
        file_filter: target
            .file_filter
            .as_deref()
            .map(crate::display::clean_path_string),
        resolved: build_search_hit_output(
            project_root,
            &target.selected,
            Some(&target.requested),
            false,
            occurrences_for_hit(occurrences, &target.selected),
        ),
        alternatives: target
            .alternatives
            .iter()
            .skip(1)
            .map(|hit| {
                build_search_hit_output(
                    project_root,
                    hit,
                    Some(&target.requested),
                    false,
                    occurrences_for_hit(occurrences, hit),
                )
            })
            .collect(),
    }
}

pub(crate) fn build_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    query: Option<&str>,
    explain: bool,
    occurrences: &[SourceOccurrenceDto],
) -> SearchHitOutput {
    let file_path = hit
        .file_path
        .as_deref()
        .map(|value| crate::display::relative_path(project_root, value));
    let score_breakdown = hit.score_breakdown.clone();
    let why = if explain {
        explain_search_hit(hit, score_breakdown.as_ref())
    } else {
        Vec::new()
    };
    let mut verification_targets =
        verification_targets_for_hit(project_root, &hit.display_name, occurrences);
    verification_targets.extend(implementation_counterpart_targets_for_hit(
        project_root,
        &hit.display_name,
        hit.file_path.as_deref(),
    ));
    verification_targets.extend(interface_implementation_targets_for_hit(
        project_root,
        &hit.display_name,
        hit.file_path.as_deref(),
    ));
    dedupe_verification_targets(&mut verification_targets);
    let primary_occurrence_kind =
        primary_occurrence(occurrences).map(|occurrence| occurrence.kind.clone());
    let symbol_role = primary_occurrence_kind
        .as_deref()
        .map(symbol_role_for_occurrence_kind)
        .map(str::to_string);
    let paired_refs = paired_occurrence_targets(
        project_root,
        &hit.display_name,
        primary_occurrence_kind.as_deref(),
        occurrences,
    );
    let resolution_hints = resolution_hints_for_hit(hit, &verification_targets, &paired_refs);
    SearchHitOutput {
        number: None,
        node_id: hit.node_id.0.clone(),
        node_ref: crate::output::node_ref(
            project_root,
            hit.file_path.as_deref(),
            hit.line,
            &hit.display_name,
        ),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path,
        line: hit.line,
        score: hit.score,
        origin: hit.origin,
        match_quality: hit
            .match_quality
            .unwrap_or_else(|| search_match_quality(query, hit)),
        resolvable: hit.resolvable,
        evidence_tier: hit.evidence_tier,
        evidence_producer: hit.evidence_producer.clone(),
        resolution_status: hit.resolution_status,
        eligible_for_sufficiency: hit.eligible_for_sufficiency,
        score_breakdown,
        duplicate_of: None,
        excerpt: repo_text_excerpt(project_root, hit),
        primary_occurrence_kind,
        symbol_role,
        paired_refs,
        verification_targets,
        resolution_hints,
        why,
    }
}

pub(super) fn build_numbered_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    query: Option<&str>,
    number: usize,
) -> SearchHitOutput {
    let mut output = build_search_hit_output(project_root, hit, query, false, &[]);
    output.number = Some(number);
    output
}

pub(super) fn search_match_quality(query: Option<&str>, hit: &SearchHit) -> SearchMatchQualityDto {
    if hit.is_text_match() {
        return SearchMatchQualityDto::RepoText;
    }
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return SearchMatchQualityDto::SemanticSuggestion;
    };
    let query_normalized = codestory_runtime::normalize_symbol_query(query);
    let display_normalized = codestory_runtime::normalize_symbol_query(&hit.display_name);
    let terminal = codestory_runtime::terminal_symbol_segment(&hit.display_name);
    let leading = codestory_runtime::leading_symbol_segment(&hit.display_name);
    if hit.display_name == query {
        return SearchMatchQualityDto::Exact;
    }
    if display_normalized == query_normalized
        || terminal == query_normalized
        || leading == query_normalized
    {
        return SearchMatchQualityDto::NormalizedExact;
    }
    if display_normalized.starts_with(&query_normalized)
        || terminal.starts_with(&query_normalized)
        || leading.starts_with(&query_normalized)
    {
        return SearchMatchQualityDto::Prefix;
    }
    if hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.semantic > 0.0 && breakdown.lexical <= f32::EPSILON)
    {
        return SearchMatchQualityDto::SemanticSuggestion;
    }
    SearchMatchQualityDto::Fuzzy
}

pub(super) fn collect_search_hit_occurrences<'a>(
    runtime: &RuntimeContext,
    hits: impl Iterator<Item = &'a SearchHit>,
) -> HashMap<NodeId, Vec<SourceOccurrenceDto>> {
    let mut seen = HashSet::new();
    let mut occurrences_by_node = HashMap::new();
    for hit in hits {
        if hit.is_text_match() || !hit.resolvable || !seen.insert(hit.node_id.clone()) {
            continue;
        }
        if let Ok(occurrences) = runtime.browser.node_occurrences(NodeOccurrencesRequest {
            id: hit.node_id.clone(),
        }) {
            occurrences_by_node.insert(hit.node_id.clone(), occurrences);
        }
    }
    occurrences_by_node
}

pub(super) fn occurrences_for_hit<'a>(
    occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    hit: &SearchHit,
) -> &'a [SourceOccurrenceDto] {
    occurrences_by_node
        .get(&hit.node_id)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub(super) fn primary_occurrence(
    occurrences: &[SourceOccurrenceDto],
) -> Option<&SourceOccurrenceDto> {
    occurrences.iter().max_by(|left, right| {
        occurrence_kind_rank(&left.kind)
            .cmp(&occurrence_kind_rank(&right.kind))
            .then_with(|| right.start_line.cmp(&left.start_line))
            .then_with(|| right.start_col.cmp(&left.start_col))
    })
}

pub(super) fn occurrence_kind_rank(kind: &str) -> u8 {
    match kind {
        "definition" | "macro_definition" => 5,
        "declaration" => 4,
        "reference" | "macro_reference" => 2,
        _ => 1,
    }
}

pub(super) fn symbol_role_for_occurrence_kind(kind: &str) -> &'static str {
    match kind {
        "definition" | "macro_definition" => "definition",
        "declaration" => "declaration",
        "reference" | "macro_reference" => "reference",
        _ => "unknown",
    }
}

pub(super) fn verification_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    occurrences: &[SourceOccurrenceDto],
) -> Vec<VerificationTargetOutput> {
    let Some(primary) = primary_occurrence(occurrences) else {
        return Vec::new();
    };
    let mut ordered = occurrences.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        occurrence_kind_rank(&right.kind)
            .cmp(&occurrence_kind_rank(&left.kind))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.start_col.cmp(&right.start_col))
    });

    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for occurrence in ordered {
        let role = symbol_role_for_occurrence_kind(&occurrence.kind);
        let is_primary = same_source_occurrence(primary, occurrence);
        if !is_primary && !matches!(role, "definition" | "declaration") {
            continue;
        }
        let key = (
            role.to_string(),
            occurrence.file_path.clone(),
            occurrence.start_line,
        );
        if !seen.insert(key) {
            continue;
        }
        let reason = if is_primary {
            "primary source occurrence selected for this symbol"
        } else if role == "definition" {
            "paired definition/body location for a declaration-style hit"
        } else {
            "paired declaration location for a definition-style hit"
        };
        targets.push(verification_target_from_occurrence(
            project_root,
            display_name,
            occurrence,
            role,
            reason,
        ));
        if targets.len() >= 4 {
            break;
        }
    }
    targets
}

pub(super) fn implementation_counterpart_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    file_path: Option<&str>,
) -> Vec<VerificationTargetOutput> {
    let Some(file_path) = file_path else {
        return Vec::new();
    };
    if !display_name.contains("::") || !is_cxx_header_path(file_path) {
        return Vec::new();
    }
    let hit_path = std::path::Path::new(file_path);
    let absolute_header = if hit_path.is_absolute() {
        hit_path.to_path_buf()
    } else {
        project_root.join(hit_path)
    };
    let Some(stem) = absolute_header.file_stem().and_then(|stem| stem.to_str()) else {
        return Vec::new();
    };
    let Some(parent) = absolute_header.parent() else {
        return Vec::new();
    };
    [".cpp", ".cc", ".cxx", ".c"]
        .into_iter()
        .filter_map(|extension| {
            let candidate = parent.join(format!("{stem}{extension}"));
            let content = fs::read_to_string(&candidate).ok()?;
            let line_index = content
                .lines()
                .position(|line| line.contains(display_name))?;
            let path =
                crate::display::relative_path(project_root, candidate.to_string_lossy().as_ref());
            let line = (line_index + 1) as u32;
            Some(VerificationTargetOutput {
                role: "definition".to_string(),
                path: path.clone(),
                line,
                node_ref: Some(format!("{path}:{line}:{display_name}")),
                reason: "sibling implementation location for a C/C++ header hit".to_string(),
            })
        })
        .collect()
}

pub(super) fn interface_implementation_targets_for_hit(
    project_root: &std::path::Path,
    display_name: &str,
    file_path: Option<&str>,
) -> Vec<VerificationTargetOutput> {
    let Some(file_path) = file_path else {
        return Vec::new();
    };
    if !is_cxx_header_path(file_path) {
        return Vec::new();
    }
    let Some((interface_name, member_name)) = split_qualified_member(display_name) else {
        return Vec::new();
    };
    let hit_path = std::path::Path::new(file_path);
    let absolute_header = if hit_path.is_absolute() {
        hit_path.to_path_buf()
    } else {
        project_root.join(hit_path)
    };
    let Ok(interface_content) = fs::read_to_string(&absolute_header) else {
        return Vec::new();
    };
    if !abstract_header_declares_member(&interface_content, interface_name, member_name) {
        return Vec::new();
    }
    let Some(parent) = absolute_header.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut headers = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path != &absolute_header && is_cxx_header_path(&path.to_string_lossy()))
        .collect::<Vec<_>>();
    headers.sort();

    let mut targets = Vec::new();
    for header in headers {
        let Some(class_name) = header.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(header_content) = fs::read_to_string(&header) else {
            continue;
        };
        if !header_declares_public_base(&header_content, class_name, interface_name) {
            continue;
        }
        let declaration_line =
            line_containing(&header_content, &format!("class {class_name}")).unwrap_or(1);
        let header_path = crate::display::relative_path(project_root, &header.to_string_lossy());
        targets.push(VerificationTargetOutput {
            role: "declaration".to_string(),
            path: header_path.clone(),
            line: declaration_line,
            node_ref: Some(format!("{header_path}:{declaration_line}:{class_name}")),
            reason: "C/C++ implementation class declaration for an abstract interface hit"
                .to_string(),
        });

        for extension in [".cpp", ".cc", ".cxx", ".c"] {
            let implementation = parent.join(format!("{class_name}{extension}"));
            let Ok(implementation_content) = fs::read_to_string(&implementation) else {
                continue;
            };
            let definition_pattern = format!("{class_name}::{member_name}");
            let Some(definition_line) =
                line_containing(&implementation_content, &definition_pattern)
            else {
                continue;
            };
            let path =
                crate::display::relative_path(project_root, &implementation.to_string_lossy());
            targets.push(VerificationTargetOutput {
                role: "definition".to_string(),
                path: path.clone(),
                line: definition_line,
                node_ref: Some(format!("{path}:{definition_line}:{definition_pattern}")),
                reason: "C/C++ implementation method for an abstract interface hit".to_string(),
            });
            break;
        }
        if targets.len() >= 4 {
            break;
        }
    }
    targets
}

pub(super) fn split_qualified_member(display_name: &str) -> Option<(&str, &str)> {
    let (owner, member) = display_name.rsplit_once("::")?;
    let owner = owner.rsplit("::").next()?.trim();
    let member = member
        .split_once('(')
        .map(|(prefix, _)| prefix)
        .unwrap_or(member)
        .trim();
    (!owner.is_empty() && !member.is_empty()).then_some((owner, member))
}

pub(super) fn abstract_header_declares_member(
    content: &str,
    interface_name: &str,
    member_name: &str,
) -> bool {
    content.contains(&format!("class {interface_name}"))
        && content.contains(member_name)
        && content.contains("= 0")
}

pub(super) fn header_declares_public_base(
    content: &str,
    class_name: &str,
    base_name: &str,
) -> bool {
    content.contains(&format!("class {class_name}"))
        && content.contains(&format!("public {base_name}"))
}

pub(super) fn line_containing(content: &str, pattern: &str) -> Option<u32> {
    content
        .lines()
        .position(|line| line.contains(pattern))
        .map(|index| (index + 1) as u32)
}

pub(super) fn is_cxx_header_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.ends_with(".h")
        || path.ends_with(".hpp")
        || path.ends_with(".hh")
        || path.ends_with(".hxx")
}

pub(super) fn paired_occurrence_targets(
    project_root: &std::path::Path,
    display_name: &str,
    primary_kind: Option<&str>,
    occurrences: &[SourceOccurrenceDto],
) -> Vec<VerificationTargetOutput> {
    let primary_role = primary_kind.map(symbol_role_for_occurrence_kind);
    let wanted_role = match primary_role {
        Some("declaration") => Some("definition"),
        Some("definition") => Some("declaration"),
        _ => None,
    };
    let Some(wanted_role) = wanted_role else {
        return Vec::new();
    };

    occurrences
        .iter()
        .filter(|occurrence| symbol_role_for_occurrence_kind(&occurrence.kind) == wanted_role)
        .take(3)
        .map(|occurrence| {
            let reason = if wanted_role == "definition" {
                "paired definition/body location"
            } else {
                "paired declaration location"
            };
            verification_target_from_occurrence(
                project_root,
                display_name,
                occurrence,
                wanted_role,
                reason,
            )
        })
        .collect()
}

pub(super) fn verification_target_from_occurrence(
    project_root: &std::path::Path,
    display_name: &str,
    occurrence: &SourceOccurrenceDto,
    role: &str,
    reason: &str,
) -> VerificationTargetOutput {
    let path = crate::display::relative_path(project_root, &occurrence.file_path);
    VerificationTargetOutput {
        role: role.to_string(),
        path: path.clone(),
        line: occurrence.start_line,
        node_ref: Some(format!("{path}:{}:{display_name}", occurrence.start_line)),
        reason: reason.to_string(),
    }
}

pub(super) fn same_source_occurrence(
    left: &SourceOccurrenceDto,
    right: &SourceOccurrenceDto,
) -> bool {
    left.kind == right.kind
        && left.file_path == right.file_path
        && left.start_line == right.start_line
        && left.start_col == right.start_col
        && left.end_line == right.end_line
        && left.end_col == right.end_col
}

pub(super) fn resolution_hints_for_hit(
    hit: &SearchHit,
    verification_targets: &[VerificationTargetOutput],
    paired_refs: &[VerificationTargetOutput],
) -> Vec<String> {
    let mut hints = Vec::new();
    if hit.kind == NodeKind::UNKNOWN {
        hints.push(
            "node kind is unknown; prefer a typed alternative for symbol/trail/snippet follow-up"
                .to_string(),
        );
    }
    if hit.is_text_match() {
        hints.push(
            "repo-text hit is a file/line hint only; choose an indexed symbol before graph browsing"
                .to_string(),
        );
        if hit
            .file_path
            .as_deref()
            .is_some_and(|path| path.ends_with(".svelte"))
        {
            hints.push(
                "Svelte files are currently surfaced through repo-text hints; typed graph edges may be unavailable for this file"
                    .to_string(),
            );
        }
    }
    if hit.resolvable && verification_targets.is_empty() {
        hints.push(
            "no source occurrence metadata was available for verification targeting".to_string(),
        );
    }
    if !paired_refs.is_empty() {
        hints.push("declaration/definition pair detected; open both files before trusting architecture claims".to_string());
    }
    hints
}

pub(super) fn explain_search_hit(
    hit: &SearchHit,
    breakdown: Option<&RetrievalScoreBreakdownDto>,
) -> Vec<String> {
    let mut why = Vec::new();
    match breakdown {
        Some(breakdown) => why.push(format!(
            "ranked by hybrid score lexical={:.3} semantic={:.3} graph={:.3} total={:.3}",
            breakdown.lexical, breakdown.semantic, breakdown.graph, breakdown.total
        )),
        None if hit.is_text_match() => why.push(
            "repo-text diagnostic match; use the file/line hint for navigation, then resolve a typed symbol before using graph evidence"
                .to_string(),
        ),
        None => why.push(format!(
            "ranked by symbolic score {:.3} with origin {}",
            hit.score,
            hit.origin.as_str()
        )),
    }
    if hit.resolvable {
        why.push("can be passed to symbol, trail, snippet, or explore as a focus id".to_string());
    }
    why
}

pub(super) fn search_query_hints(
    query: &str,
    indexed_hits: &[SearchHitOutput],
    repo_text_hits: &[SearchHitOutput],
) -> Vec<String> {
    if !indexed_hits.is_empty() {
        return Vec::new();
    }
    let mut hints = Vec::new();
    if repo_text_hits.is_empty() {
        hints.push(
            "No indexed symbol or repo-text hits; try a shorter symbol name, module path, or run index --refresh full."
                .to_string(),
        );
    } else {
        hints.push(
            "Only repo-text hits matched; try a concrete identifier from an excerpt to resolve a symbol."
                .to_string(),
        );
    }
    let terms = query
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|term| term.len() >= 3)
        .take(4)
        .collect::<Vec<_>>();
    if !terms.is_empty() {
        hints.push(format!("Possible query terms: {}", terms.join(", ")));
    }
    hints
}

pub(super) fn search_hit_location_key(hit: &SearchHitOutput) -> Option<(String, u32)> {
    Some((hit.file_path.clone()?, hit.line?))
}

#[cfg(test)]
pub(super) fn hide_speculative_trail_edges(mut context: TrailContextDto) -> TrailContextDto {
    let original_edge_count = context.trail.edges.len();
    let retained_edges = context
        .trail
        .edges
        .into_iter()
        .filter(|edge| !is_speculative_trail_edge(edge))
        .collect::<Vec<_>>();

    let mut adjacency = HashMap::new();
    for edge in &retained_edges {
        adjacency
            .entry(edge.source.clone())
            .or_insert_with(Vec::new)
            .push(edge.target.clone());
        adjacency
            .entry(edge.target.clone())
            .or_insert_with(Vec::new)
            .push(edge.source.clone());
    }

    let mut reachable = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    reachable.insert(context.trail.center_id.clone());
    queue.push_back(context.trail.center_id.clone());
    while let Some(node_id) = queue.pop_front() {
        if let Some(next_nodes) = adjacency.get(&node_id) {
            for next in next_nodes {
                if reachable.insert(next.clone()) {
                    queue.push_back(next.clone());
                }
            }
        }
    }

    context
        .trail
        .nodes
        .retain(|node| reachable.contains(&node.id));
    context.trail.edges = retained_edges
        .into_iter()
        .filter(|edge| reachable.contains(&edge.source) && reachable.contains(&edge.target))
        .collect();
    let omitted_edges = original_edge_count.saturating_sub(context.trail.edges.len()) as u32;
    context.trail.omitted_edge_count = context
        .trail
        .omitted_edge_count
        .saturating_add(omitted_edges);

    if let Some(layout) = context.trail.canonical_layout.as_mut() {
        layout.nodes.retain(|node| reachable.contains(&node.id));
        layout.edges.retain(|edge| {
            !is_speculative_certainty_label(edge.certainty.as_deref())
                && reachable.contains(&edge.source)
                && reachable.contains(&edge.target)
        });
    }

    context
}

#[cfg(test)]
pub(super) fn is_speculative_trail_edge(edge: &codestory_contracts::api::GraphEdgeDto) -> bool {
    if is_speculative_certainty_label(edge.certainty.as_deref()) {
        return true;
    }
    is_runtime_bridge_edge(edge.kind)
        && (is_probable_certainty_label(edge.certainty.as_deref())
            || edge.confidence.is_some_and(|confidence| {
                confidence < codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN
            }))
}

#[cfg(test)]
pub(super) fn is_speculative_certainty_label(certainty: Option<&str>) -> bool {
    matches!(
        certainty.map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("uncertain" | "speculative")
    )
}

#[cfg(test)]
pub(super) fn is_probable_certainty_label(certainty: Option<&str>) -> bool {
    certainty
        .map(|value| value.eq_ignore_ascii_case("probable"))
        .unwrap_or(false)
}

#[cfg(test)]
pub(super) fn is_runtime_bridge_edge(kind: codestory_contracts::api::EdgeKind) -> bool {
    matches!(
        kind,
        codestory_contracts::api::EdgeKind::CALL | codestory_contracts::api::EdgeKind::MACRO_USAGE
    )
}

pub(super) fn repo_text_excerpt(project_root: &std::path::Path, hit: &SearchHit) -> Option<String> {
    if !hit.is_text_match() {
        return None;
    }
    let path = std::path::Path::new(hit.file_path.as_deref()?);
    let line = hit.line?;
    let resolved_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let contents = fs::read_to_string(resolved_path).ok()?;
    let source_line = contents
        .lines()
        .nth(line.saturating_sub(1) as usize)?
        .trim();
    Some(compact_excerpt(source_line, 140))
}

pub(super) fn compact_excerpt(line: &str, max_len: usize) -> String {
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= max_len {
        return collapsed;
    }
    let clipped = collapsed
        .char_indices()
        .take_while(|(idx, _)| *idx < max_len.saturating_sub(1))
        .map(|(_, ch)| ch)
        .collect::<String>();
    format!("{clipped}…")
}
