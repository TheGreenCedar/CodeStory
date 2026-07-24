use super::search::dedupe_verification_targets;
use crate::args::{SearchHitOutput, VerificationTargetOutput};
use crate::runtime::RuntimeContext;
use codestory_contracts::api::{
    NodeId, NodeKind, NodeOccurrencesRequest, RetrievalScoreBreakdownDto, SearchHit,
    SearchMatchQualityDto, SourceOccurrenceDto,
};
use std::collections::{HashMap, HashSet};

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
    verification_targets.extend(hit.verification_targets.iter().map(|target| {
        let path = crate::display::relative_path(project_root, &target.file_path);
        VerificationTargetOutput {
            role: target.role.clone(),
            path: path.clone(),
            line: target.line,
            node_ref: Some(format!("{path}:{}:{}", target.line, target.display_name)),
            reason: target.reason.clone(),
        }
    }));
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
        excerpt: if hit.is_text_match() {
            hit.source_excerpt
                .as_deref()
                .map(|excerpt| compact_excerpt(excerpt.trim(), 140))
        } else {
            None
        },
        primary_occurrence_kind,
        symbol_role,
        paired_refs,
        verification_targets,
        resolution_hints,
        why,
    }
}

pub(in crate::app) fn build_numbered_search_hit_output(
    project_root: &std::path::Path,
    hit: &SearchHit,
    query: Option<&str>,
    number: usize,
) -> SearchHitOutput {
    let mut output = build_search_hit_output(project_root, hit, query, false, &[]);
    output.number = Some(number);
    output
}

pub(in crate::app) fn search_match_quality(
    query: Option<&str>,
    hit: &SearchHit,
) -> SearchMatchQualityDto {
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

pub(in crate::app) fn collect_search_hit_occurrences<'a>(
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

pub(in crate::app) fn occurrences_for_hit<'a>(
    occurrences_by_node: &'a HashMap<NodeId, Vec<SourceOccurrenceDto>>,
    hit: &SearchHit,
) -> &'a [SourceOccurrenceDto] {
    occurrences_by_node
        .get(&hit.node_id)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

pub(in crate::app) fn primary_occurrence(
    occurrences: &[SourceOccurrenceDto],
) -> Option<&SourceOccurrenceDto> {
    occurrences.iter().max_by(|left, right| {
        occurrence_kind_rank(&left.kind)
            .cmp(&occurrence_kind_rank(&right.kind))
            .then_with(|| right.start_line.cmp(&left.start_line))
            .then_with(|| right.start_col.cmp(&left.start_col))
    })
}

pub(in crate::app) fn occurrence_kind_rank(kind: &str) -> u8 {
    match kind {
        "definition" | "macro_definition" => 5,
        "declaration" => 4,
        "reference" | "macro_reference" => 2,
        _ => 1,
    }
}

pub(in crate::app) fn symbol_role_for_occurrence_kind(kind: &str) -> &'static str {
    match kind {
        "definition" | "macro_definition" => "definition",
        "declaration" => "declaration",
        "reference" | "macro_reference" => "reference",
        _ => "unknown",
    }
}

pub(in crate::app) fn verification_targets_for_hit(
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

pub(in crate::app) fn paired_occurrence_targets(
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

pub(in crate::app) fn verification_target_from_occurrence(
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

pub(in crate::app) fn same_source_occurrence(
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

pub(in crate::app) fn resolution_hints_for_hit(
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

pub(in crate::app) fn explain_search_hit(
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

pub(in crate::app) fn compact_excerpt(line: &str, max_len: usize) -> String {
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
