use anyhow::{Context, Result};
use codestory_contracts::api::{
    ApiError, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeKind, PacketEvidenceResolutionDto,
    PacketEvidenceTierDto, SearchHit, SearchHitOrigin, SearchMatchQualityDto,
};
use serde::Serialize;
use std::fs;
use std::path::Path;

use crate::agent::packet_evidence::{decorate_search_hit_evidence, diagnostic_source_evidence};
use crate::{
    AppController, compare_ranked_hits, retrieval_file_role_for_hit, symbol_name_match_rank,
};

const HUMAN_AMBIGUOUS_ALTERNATIVE_LIMIT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSelector {
    Id,
    Query,
}

#[derive(Debug, Clone)]
pub enum TargetSelection {
    Id(NodeId),
    Query {
        query: String,
        choose: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTarget {
    pub selector: TargetSelector,
    pub requested: String,
    pub file_filter: Option<String>,
    pub selected: SearchHit,
    pub alternatives: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
pub struct AmbiguousTarget {
    pub query: String,
    pub file_filter: Option<String>,
    pub alternatives: Vec<SearchHit>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum TargetResolution {
    Resolved(Box<ResolvedTarget>),
    Ambiguous(AmbiguousTarget),
    Rejected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ResolutionCandidateRank {
    file_filter_match: u8,
    resolution: ResolutionRank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ResolutionRank {
    collection_definition_path: u8,
    concrete_exact_anchor: u8,
    exact_display: u8,
    exact_terminal: u8,
    exact_leading: u8,
    exact_case_match: u8,
    source_truth_bucket: u8,
    inexact_query_prefix_match: u8,
    implementation_path: u8,
    type_definition_line: u8,
    callable_definition_line: u8,
    declaration_anchor: u8,
    kind_bucket: u8,
}

pub(crate) fn compare_resolution_hits(
    query: &str,
    left: &SearchHit,
    right: &SearchHit,
) -> std::cmp::Ordering {
    compare_ranked_hits(
        left,
        right,
        resolution_rank(query, left),
        resolution_rank(query, right),
    )
}

pub(crate) fn resolution_rank(query: &str, hit: &SearchHit) -> ResolutionRank {
    resolution_rank_with_project_root(None, query, hit)
}

pub(crate) fn resolution_rank_with_project_root(
    project_root: Option<&Path>,
    query: &str,
    hit: &SearchHit,
) -> ResolutionRank {
    let rank = symbol_name_match_rank(query, &hit.display_name);

    ResolutionRank {
        collection_definition_path: collection_definition_path_bucket(query, hit),
        concrete_exact_anchor: concrete_exact_anchor_bucket(query, hit),
        exact_display: rank.exact_display,
        exact_terminal: rank.exact_terminal,
        exact_leading: rank.exact_leading,
        exact_case_match: exact_case_match_bucket(query, hit),
        source_truth_bucket: source_truth_bucket(hit),
        inexact_query_prefix_match: inexact_query_prefix_match_bucket(query, hit),
        implementation_path: implementation_path_bucket(hit),
        type_definition_line: type_definition_line_bucket(project_root, query, hit),
        callable_definition_line: callable_definition_line_bucket(project_root, query, hit),
        declaration_anchor: declaration_anchor_bucket(hit),
        kind_bucket: resolution_kind_bucket(hit.kind),
    }
}

pub(crate) fn is_graph_target_candidate(hit: &SearchHit) -> bool {
    !matches!(
        hit.match_quality,
        Some(SearchMatchQualityDto::SemanticSuggestion | SearchMatchQualityDto::RepoText)
    ) && hit.evidence_tier != Some(PacketEvidenceTierDto::StructuralText)
        && !matches!(
            hit.resolution_status,
            Some(
                PacketEvidenceResolutionDto::SourceRangeOnly
                    | PacketEvidenceResolutionDto::Unresolved
                    | PacketEvidenceResolutionDto::DiagnosticOnly
            )
        )
}

pub(crate) fn is_name_resolvable_graph_target(query: &str, hit: &SearchHit) -> bool {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0 {
        return true;
    }
    if inexact_query_prefix_match_bucket(query, hit) != 0 {
        return true;
    }

    let query = crate::normalize_symbol_query(query);
    if query.is_empty() {
        return false;
    }
    let display = crate::normalize_symbol_query(&hit.display_name);
    if query.contains('/') && display.contains(&query) {
        return true;
    }
    let terminal = crate::terminal_symbol_segment(&hit.display_name);
    let leading = crate::leading_symbol_segment(&hit.display_name);
    display.starts_with(&query) || terminal.starts_with(&query) || leading.starts_with(&query)
}

pub(crate) fn is_resolvable_graph_target(query: &str, hit: &SearchHit) -> bool {
    hit.resolvable && is_graph_target_candidate(hit) && is_name_resolvable_graph_target(query, hit)
}

fn inexact_query_prefix_match_bucket(query: &str, hit: &SearchHit) -> u8 {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0 {
        return 0;
    }
    let query = crate::normalize_symbol_query(query);
    let terminal = crate::terminal_symbol_segment(&hit.display_name);
    if terminal.len() < 4 || query == terminal {
        return 0;
    }

    u8::from(
        query.starts_with(&terminal)
            && query
                .as_bytes()
                .get(terminal.len())
                .is_some_and(|byte| matches!(*byte, b'_' | b'-')),
    )
}

fn source_truth_bucket(hit: &SearchHit) -> u8 {
    if is_non_primary_or_generated_hit(hit) {
        0
    } else {
        1
    }
}

fn is_non_primary_or_generated_hit(hit: &SearchHit) -> bool {
    retrieval_file_role_for_hit(hit).is_non_primary()
}

fn exact_case_match_bucket(query: &str, hit: &SearchHit) -> u8 {
    if hit.display_name == query || terminal_segment_raw(&hit.display_name) == query {
        return 2;
    }
    if leading_segment_raw(&hit.display_name) == query {
        return 1;
    }
    0
}

fn concrete_exact_anchor_bucket(query: &str, hit: &SearchHit) -> u8 {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }

    match hit.kind {
        NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ANNOTATION
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 4,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => 3,
        NodeKind::GLOBAL_VARIABLE | NodeKind::CONSTANT => 2,
        NodeKind::FIELD | NodeKind::VARIABLE | NodeKind::ENUM_CONSTANT => 1,
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::FILE
        | NodeKind::TYPE_PARAMETER
        | NodeKind::BUILTIN_TYPE
        | NodeKind::UNKNOWN => 0,
    }
}

fn terminal_segment_raw(value: &str) -> &str {
    value.rsplit([':', '.', '/', '\\']).next().unwrap_or(value)
}

fn leading_segment_raw(value: &str) -> &str {
    value.split("::").next().unwrap_or(value)
}

fn collection_definition_path_bucket(query: &str, hit: &SearchHit) -> u8 {
    if !matches!(hit.kind, NodeKind::GLOBAL_VARIABLE | NodeKind::CONSTANT) {
        return 0;
    }
    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }
    hit.file_path
        .as_deref()
        .map(|path| {
            let path = normalize_path_fragment(path);
            u8::from(path.contains("/collections/") && !path.contains("generated"))
        })
        .unwrap_or(0)
}

fn implementation_path_bucket(hit: &SearchHit) -> u8 {
    let Some(path) = hit.file_path.as_deref() else {
        return 0;
    };
    let path = normalize_path_fragment(path);
    if path.ends_with("/services.rs")
        || path.ends_with("/browser.rs")
        || path.ends_with("/http_transport.rs")
        || path.ends_with("/stdio_transport.rs")
    {
        0
    } else {
        1
    }
}

pub(crate) fn search_hit_matches_file_filter(
    project_root: &Path,
    hit: &SearchHit,
    fragment: &str,
) -> bool {
    file_filter_match_bucket(project_root, hit, fragment) > 0
}

pub(crate) fn file_filter_match_bucket(project_root: &Path, hit: &SearchHit, fragment: &str) -> u8 {
    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };

    let absolute = normalize_path_fragment(file_path);
    let relative = normalize_path_fragment(&relative_path(project_root, file_path));
    let fragment = normalize_path_fragment(fragment);
    let fragment = fragment.trim_matches('/').to_string();
    if fragment.is_empty() {
        return 0;
    }

    if relative == fragment || absolute == fragment {
        return 4;
    }

    if relative.ends_with(&format!("/{fragment}")) || absolute.ends_with(&format!("/{fragment}")) {
        return 3;
    }

    if relative
        .rsplit('/')
        .next()
        .is_some_and(|file_name| file_name == fragment)
    {
        return 2;
    }

    if relative.contains(&fragment) || absolute.contains(&fragment) {
        return 1;
    }

    0
}

fn resolution_kind_bucket(kind: NodeKind) -> u8 {
    if matches!(
        kind,
        NodeKind::MODULE
            | NodeKind::NAMESPACE
            | NodeKind::PACKAGE
            | NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) {
        return 2;
    }

    if matches!(
        kind,
        NodeKind::FUNCTION
            | NodeKind::METHOD
            | NodeKind::MACRO
            | NodeKind::FIELD
            | NodeKind::VARIABLE
            | NodeKind::GLOBAL_VARIABLE
            | NodeKind::CONSTANT
            | NodeKind::ENUM_CONSTANT
    ) {
        return 1;
    }

    0
}

fn normalize_path_fragment(value: &str) -> String {
    clean_path_string(value).to_ascii_lowercase()
}

fn clean_path_string(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{stripped}");
    } else if normalized.starts_with("//?/") {
        normalized = normalized[4..].to_string();
    }
    normalized
}

fn relative_path(project_root: &Path, raw: &str) -> String {
    let normalized = clean_path_string(raw);
    codestory_workspace::workspace_relative_path(project_root, Path::new(&normalized))
        .map(|path| clean_path_string(&path.to_string_lossy()))
        .unwrap_or(normalized)
}

fn declaration_anchor_bucket(hit: &SearchHit) -> u8 {
    if matches!(
        hit.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) && !hit_is_impl_anchor(hit)
    {
        return 1;
    }

    0
}

fn type_definition_line_bucket(project_root: Option<&Path>, query: &str, hit: &SearchHit) -> u8 {
    if !matches!(
        hit.kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
    ) {
        return 0;
    }

    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }

    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };
    let Some(line) = hit.line else {
        return 0;
    };
    let Ok(contents) = read_file_contents_for_resolution(project_root, file_path) else {
        return 0;
    };
    let Some(source_line) = contents.lines().nth(line.saturating_sub(1) as usize) else {
        return 0;
    };
    let trimmed = source_line.split("//").next().unwrap_or(source_line).trim();
    let expected = crate::terminal_symbol_segment(query);
    let tokens = trimmed
        .split(|ch: char| ch.is_whitespace() || ch == ':' || ch == ';' || ch == '{')
        .map(|token| token.trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let Some(keyword_index) = tokens
        .iter()
        .position(|token| matches!(*token, "class" | "struct" | "interface" | "enum" | "union"))
    else {
        return 0;
    };
    let Some(type_name) = tokens.get(keyword_index + 1).copied() else {
        return 0;
    };
    if !type_name.eq_ignore_ascii_case(&expected) {
        return 0;
    }
    if trimmed.contains('{') || !trimmed.ends_with(';') {
        2
    } else {
        0
    }
}

fn callable_definition_line_bucket(
    project_root: Option<&Path>,
    query: &str,
    hit: &SearchHit,
) -> u8 {
    if !matches!(
        hit.kind,
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
    ) {
        return 0;
    }

    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display == 0 && rank.exact_terminal == 0 && rank.exact_leading == 0 {
        return 0;
    }

    let Some(file_path) = hit.file_path.as_deref() else {
        return 0;
    };
    let Some(line) = hit.line else {
        return 0;
    };
    let Ok(contents) = read_file_contents_for_resolution(project_root, file_path) else {
        return 0;
    };
    let line_index = line.saturating_sub(1) as usize;
    let Some(source_line) = contents.lines().nth(line_index) else {
        return 0;
    };
    let trimmed = source_line.split("//").next().unwrap_or(source_line).trim();
    let expected = crate::terminal_symbol_segment(query);
    if expected.is_empty() || !line_contains_symbol_name(trimmed, &expected) {
        return 0;
    }
    let signature_window = contents
        .lines()
        .skip(line_index)
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    if looks_like_callable_declaration(&signature_window) {
        return 0;
    }
    if !looks_like_callable_definition(&signature_window) {
        return 0;
    }

    2
}

fn line_contains_symbol_name(line: &str, expected: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|token| token.eq_ignore_ascii_case(expected))
}

fn looks_like_callable_declaration(line: &str) -> bool {
    let brace = line.find('{');
    let semicolon = line.find(';');
    let before_body = brace.map(|index| &line[..index]).unwrap_or(line);
    matches!(
        (brace, semicolon),
        (Some(brace), Some(semicolon)) if semicolon < brace
    ) || matches!((brace, semicolon), (None, Some(_)))
        || before_body.contains("= 0;")
}

fn looks_like_callable_definition(line: &str) -> bool {
    let brace = line.find('{');
    let semicolon = line.find(';');
    matches!(
        (brace, semicolon),
        (Some(brace), Some(semicolon)) if brace < semicolon
    ) || matches!((brace, semicolon), (Some(_), None))
}

fn hit_is_impl_anchor(hit: &SearchHit) -> bool {
    let Some(file_path) = hit.file_path.as_deref() else {
        return false;
    };
    let Some(line) = hit.line else {
        return false;
    };
    let Ok(contents) = read_file_contents_for_resolution(None, file_path) else {
        return false;
    };
    let Some(source_line) = contents.lines().nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let trimmed = source_line.trim_start();
    trimmed.starts_with("impl ") || trimmed.starts_with("unsafe impl ")
}

fn read_file_contents_for_resolution(project_root: Option<&Path>, path: &str) -> Result<String> {
    let raw_path = Path::new(path);
    let joined_path;
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else if let Some(root) = project_root {
        joined_path = root.join(raw_path);
        joined_path.as_path()
    } else {
        raw_path
    };

    if let Ok(contents) = fs::read_to_string(candidate) {
        return Ok(contents);
    }

    #[cfg(windows)]
    if let Some(stripped) = path.strip_prefix(r"\\?\")
        && let Ok(contents) = fs::read_to_string(stripped)
    {
        return Ok(contents);
    }

    fs::read_to_string(path).with_context(|| format!("Failed to read file `{path}`"))
}

impl AppController {
    pub fn resolve_target(
        &self,
        target: TargetSelection,
        file_filter: Option<&str>,
    ) -> Result<TargetResolution, ApiError> {
        match target {
            TargetSelection::Id(id) => {
                let details = self.node_details(NodeDetailsRequest { id: id.clone() })?;
                Ok(resolve_id_target(id, &details))
            }
            TargetSelection::Query { query, choose } => {
                self.resolve_query_target(query, choose, file_filter)
            }
        }
    }

    fn resolve_query_target(
        &self,
        query: String,
        choose: Option<usize>,
        file_filter: Option<&str>,
    ) -> Result<TargetResolution, ApiError> {
        let project_root = self.require_project_root()?;
        let mut alternatives = self.query_resolution_alternatives(&query)?;
        if alternatives.is_empty()
            && let Some(stem) = command_query_resolution_stem(&query)
        {
            alternatives = self.query_resolution_alternatives(&stem)?;
        }
        alternatives.retain(|hit| is_resolvable_graph_target(&query, hit));
        if let Some(file_filter) = file_filter {
            alternatives
                .retain(|hit| search_hit_matches_file_filter(&project_root, hit, file_filter));
        }
        if alternatives.is_empty() {
            return Ok(TargetResolution::Rejected(no_query_match_error(
                &project_root,
                &query,
                file_filter,
            )));
        }
        alternatives.sort_by(|left, right| {
            compare_resolution_candidates(&project_root, &query, file_filter, left, right)
        });
        let tied = tied_top_alternatives(&project_root, &query, file_filter, &alternatives);
        if let Some(choice) = choose {
            if choice == 0 || choice > tied.len() {
                return Ok(TargetResolution::Rejected(format!(
                    "`--choose {choice}` is outside the displayed alternative range 1..={}. Re-run without `--choose` to inspect the current alternatives.",
                    tied.len()
                )));
            }
            let selected = tied[choice - 1].clone();
            promote_selected_alternative(&mut alternatives, &selected);
            return Ok(TargetResolution::Resolved(Box::new(query_resolved_target(
                query,
                file_filter,
                selected,
                alternatives,
            ))));
        }
        if tied.len() > 1 {
            return Ok(TargetResolution::Ambiguous(AmbiguousTarget {
                query: query.clone(),
                file_filter: file_filter.map(ToOwned::to_owned),
                message: ambiguous_query_error(&project_root, &query, file_filter, &tied),
                alternatives: tied,
            }));
        }
        debug_assert_unique_top_candidate(&project_root, &query, file_filter, &alternatives);
        let selected = alternatives
            .first()
            .cloned()
            .expect("non-empty alternatives checked above");
        Ok(TargetResolution::Resolved(Box::new(query_resolved_target(
            query,
            file_filter,
            selected,
            alternatives,
        ))))
    }

    fn query_resolution_alternatives(&self, query: &str) -> Result<Vec<SearchHit>, ApiError> {
        let mut alternatives = self.resolve_indexed_symbol_candidates(query, 50)?;
        alternatives.retain(|hit| is_resolvable_graph_target(query, hit));
        Ok(alternatives)
    }
}

fn command_query_resolution_stem(query: &str) -> Option<String> {
    ["_command", "_cmd", "_handler"]
        .into_iter()
        .find_map(|suffix| {
            query
                .strip_suffix(suffix)
                .filter(|stem| stem.len() >= 4)
                .map(ToOwned::to_owned)
        })
}

fn query_resolved_target(
    query: String,
    file_filter: Option<&str>,
    selected: SearchHit,
    alternatives: Vec<SearchHit>,
) -> ResolvedTarget {
    ResolvedTarget {
        selector: TargetSelector::Query,
        requested: query,
        file_filter: file_filter.map(ToOwned::to_owned),
        selected,
        alternatives,
    }
}

fn promote_selected_alternative(alternatives: &mut Vec<SearchHit>, selected: &SearchHit) {
    if let Some(position) = alternatives
        .iter()
        .position(|hit| hit.node_id == selected.node_id)
    {
        let selected = alternatives.remove(position);
        alternatives.insert(0, selected);
    }
}

fn tied_top_alternatives(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) -> Vec<SearchHit> {
    let Some(first) = alternatives.first() else {
        return Vec::new();
    };
    let top_rank = resolution_candidate_rank(project_root, query, file_filter, first);
    alternatives
        .iter()
        .take_while(|hit| {
            resolution_candidate_rank(project_root, query, file_filter, hit) == top_rank
        })
        .cloned()
        .collect()
}

fn debug_assert_unique_top_candidate(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) {
    if alternatives.len() > 1 {
        debug_assert_ne!(
            resolution_candidate_rank(project_root, query, file_filter, &alternatives[0]),
            resolution_candidate_rank(project_root, query, file_filter, &alternatives[1])
        );
    }
}

fn resolution_candidate_rank(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    hit: &SearchHit,
) -> ResolutionCandidateRank {
    ResolutionCandidateRank {
        file_filter_match: file_filter
            .map(|filter| file_filter_match_bucket(project_root, hit, filter))
            .unwrap_or(0),
        resolution: resolution_rank_with_project_root(Some(project_root), query, hit),
    }
}

fn compare_resolution_candidates(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    left: &SearchHit,
    right: &SearchHit,
) -> std::cmp::Ordering {
    resolution_candidate_rank(project_root, query, file_filter, right)
        .cmp(&resolution_candidate_rank(
            project_root,
            query,
            file_filter,
            left,
        ))
        .then_with(|| compare_resolution_hits(query, left, right))
        .then_with(|| left.node_id.0.cmp(&right.node_id.0))
}

fn search_hit_from_node(node: &NodeDetailsDto) -> SearchHit {
    let diagnostic_evidence =
        diagnostic_source_evidence(node.file_path.as_deref(), node.canonical_id.as_deref());
    let mut hit = SearchHit {
        node_id: node.id.clone(),
        display_name: node.display_name.clone(),
        kind: node.kind,
        file_path: node.file_path.clone(),
        line: node.start_line,
        score: 0.0,
        origin: SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: Some(
            diagnostic_evidence
                .map(|evidence| evidence.tier)
                .unwrap_or(PacketEvidenceTierDto::ResolvedGraph),
        ),
        evidence_producer: Some(
            diagnostic_evidence
                .map(|evidence| evidence.producer)
                .unwrap_or("node_details")
                .to_string(),
        ),
        resolution_status: Some(
            diagnostic_evidence
                .map(|evidence| evidence.resolution)
                .unwrap_or(PacketEvidenceResolutionDto::Resolved),
        ),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };
    decorate_search_hit_evidence(&mut hit);
    hit
}

fn resolve_id_target(id: NodeId, details: &NodeDetailsDto) -> TargetResolution {
    let selected = search_hit_from_node(details);
    if !selected.resolvable || !is_graph_target_candidate(&selected) {
        return TargetResolution::Rejected(format!(
            "id_resolution: Node `{}` is source-range-only diagnostic evidence and cannot be selected as a typed graph target. Its cited source remains available through the node and snippet surfaces.",
            id.0
        ));
    }
    TargetResolution::Resolved(Box::new(ResolvedTarget {
        selector: TargetSelector::Id,
        requested: id.0,
        file_filter: None,
        selected,
        alternatives: Vec::new(),
    }))
}

fn no_query_match_error(project_root: &Path, query: &str, file_filter: Option<&str>) -> String {
    let search_command = format!(
        "codestory-cli search --project {} --query {} --limit 10",
        quote_cli_path(project_root),
        quote_cli_value(query)
    );
    match file_filter {
        Some(file_filter) => format!(
            "query_resolution: No symbol matched query `{query}` within files matching `{}`. Run `{search_command}` to inspect candidates, or relax `--file`.",
            clean_path_string(file_filter)
        ),
        None => format!(
            "query_resolution: No symbol matched query `{query}`. Run `{search_command}` to inspect candidates."
        ),
    }
}

fn ambiguous_query_error(
    project_root: &Path,
    query: &str,
    file_filter: Option<&str>,
    alternatives: &[SearchHit],
) -> String {
    let scope = file_filter
        .map(|value| format!(" even after applying `--file {}`", clean_path_string(value)))
        .unwrap_or_default();
    let mut message = format!(
        "Query `{query}` is ambiguous{scope}; choose a match or pass a stable id.\n\nNext commands:\n"
    );
    let filter_arg = file_filter
        .map(|value| format!(" --file {}", quote_cli_value(&clean_path_string(value))))
        .unwrap_or_default();
    message.push_str(&format!(
        "  codestory-cli symbol --project {} --query {}{filter_arg} --choose 1\n",
        quote_cli_path(project_root),
        quote_cli_value(query)
    ));
    if let Some(first) = alternatives.first() {
        message.push_str(&format!(
            "  codestory-cli symbol --project {} --id {}\n",
            quote_cli_path(project_root),
            first.node_id.0
        ));
        if let Some(path) = first.file_path.as_deref() {
            message.push_str(&format!(
                "  codestory-cli symbol --project {} --query {} --file {}\n",
                quote_cli_path(project_root),
                quote_cli_value(query),
                quote_cli_value(&relative_path(project_root, path))
            ));
        }
    }
    message.push_str(if file_filter.is_some() {
        "\nPass a more qualified symbol name, a stable `--id`, or a narrower `--file` fragment."
    } else {
        "\nPass a more qualified symbol name, add `--file <path-fragment>`, or resolve the exact `--id` from `search` output."
    });
    let displayed = alternatives.len().min(HUMAN_AMBIGUOUS_ALTERNATIVE_LIMIT);
    message.push_str(&format!(
        "\n\nTop equally ranked matches (showing {displayed} of {}):\n",
        alternatives.len()
    ));
    for (index, hit) in alternatives
        .iter()
        .take(HUMAN_AMBIGUOUS_ALTERNATIVE_LIMIT)
        .enumerate()
    {
        message.push_str(&format!(
            "  {}. {} id=`{}`",
            index + 1,
            format_search_hit_target(project_root, hit),
            hit.node_id.0
        ));
        if let Some(reference) = node_ref(project_root, hit) {
            message.push_str(&format!(" ref=`{reference}`"));
        }
        message.push('\n');
    }
    message
}

fn format_search_hit_target(project_root: &Path, hit: &SearchHit) -> String {
    let mut output = format!(
        "{} [{}]",
        hit.display_name,
        format!("{:?}", hit.kind).to_ascii_lowercase()
    );
    if let Some(path) = hit.file_path.as_deref() {
        output.push(' ');
        output.push_str(&relative_path(project_root, path));
    }
    if let Some(line) = hit.line {
        output.push(':');
        output.push_str(&line.to_string());
    }
    output
}

fn node_ref(project_root: &Path, hit: &SearchHit) -> Option<String> {
    Some(format!(
        "{}:{}:{}",
        relative_path(project_root, hit.file_path.as_deref()?),
        hit.line?,
        hit.display_name
    ))
}

fn quote_cli_path(path: &Path) -> String {
    quote_cli_value(&clean_path_string(&path.to_string_lossy()))
}

fn quote_cli_value(value: &str) -> String {
    if value.chars().any(|ch| matches!(ch, '$' | '`' | '\'' | '"')) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, SearchHitOrigin};
    use tempfile::tempdir;

    fn test_search_hit_defaults() -> SearchHit {
        SearchHit {
            node_id: NodeId(String::new()),
            display_name: String::new(),
            kind: NodeKind::UNKNOWN,
            file_path: None,
            line: None,
            score: 0.0,
            origin: SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }
    }

    fn hit(id: &str, display_name: &str, kind: NodeKind, score: f32, path: &str) -> SearchHit {
        SearchHit {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: Some(path.to_string()),
            line: Some(1),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
            ..test_search_hit_defaults()
        }
    }

    #[test]
    fn semantic_suggestions_are_not_graph_target_candidates() {
        let mut semantic = hit(
            "semantic",
            "ElsewhereFeedProps",
            NodeKind::TYPEDEF,
            0.12,
            "src/components/ElsewhereFeed.tsx",
        );
        semantic.match_quality = Some(SearchMatchQualityDto::SemanticSuggestion);
        let mut repo_text = semantic.clone();
        repo_text.match_quality = Some(SearchMatchQualityDto::RepoText);
        let mut fuzzy = semantic.clone();
        fuzzy.match_quality = Some(SearchMatchQualityDto::Fuzzy);

        assert!(!is_graph_target_candidate(&semantic));
        assert!(!is_graph_target_candidate(&repo_text));
        assert!(is_graph_target_candidate(&fuzzy));
    }

    #[test]
    fn node_details_target_keeps_structural_text_evidence_explicit() {
        let details = NodeDetailsDto {
            id: NodeId("cargo-package".to_string()),
            kind: NodeKind::PACKAGE,
            display_name: "demo".to_string(),
            serialized_name: "demo".to_string(),
            qualified_name: None,
            canonical_id: None,
            file_path: Some("crates/demo/Cargo.toml".to_string()),
            start_line: Some(2),
            start_col: Some(1),
            end_line: Some(2),
            end_col: Some(8),
            member_access: None,
            route_endpoint: None,
        };
        let hit = search_hit_from_node(&details);

        assert_eq!(
            hit.evidence_tier,
            Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
        );
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("structural_cargo_manifest_collector")
        );
        assert_eq!(
            hit.resolution_status,
            Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
        assert!(hit.resolvable, "the cited source range remains navigable");
        assert!(!is_graph_target_candidate(&hit));
        assert!(!is_resolvable_graph_target("demo", &hit));
        assert!(matches!(
            resolve_id_target(details.id.clone(), &details),
            TargetResolution::Rejected(message)
                if message.contains("source-range-only diagnostic evidence")
        ));
    }

    #[test]
    fn direct_id_openapi_target_keeps_exact_source_identity_but_rejects_typed_resolution() {
        let details = NodeDetailsDto {
            id: NodeId("openapi-users".to_string()),
            kind: NodeKind::FUNCTION,
            display_name: "GET /api/users".to_string(),
            serialized_name: "GET /api/users".to_string(),
            qualified_name: None,
            canonical_id: Some("openapi:endpoint:GET /api/users".to_string()),
            file_path: Some("openapi.json".to_string()),
            start_line: Some(8),
            start_col: Some(1),
            end_line: Some(8),
            end_col: Some(16),
            member_access: None,
            route_endpoint: None,
        };
        let hit = search_hit_from_node(&details);

        assert_eq!(hit.evidence_tier, Some(PacketEvidenceTierDto::ExactSource));
        assert_eq!(
            hit.evidence_producer.as_deref(),
            Some("openapi_endpoint_schema")
        );
        assert_eq!(
            hit.resolution_status,
            Some(PacketEvidenceResolutionDto::SourceRangeOnly)
        );
        assert_eq!(hit.eligible_for_sufficiency, Some(false));
        assert!(hit.resolvable, "the cited source range remains navigable");
        assert!(!is_graph_target_candidate(&hit));
        assert!(matches!(
            resolve_id_target(details.id.clone(), &details),
            TargetResolution::Rejected(message)
                if message.contains("source-range-only diagnostic evidence")
        ));
    }

    #[test]
    fn guessed_symbol_names_do_not_resolve_to_semantic_neighbors() {
        let neighbor = hit(
            "neighbor",
            "ElsewhereFeedProps",
            NodeKind::TYPEDEF,
            0.12,
            "src/components/ElsewhereFeed.tsx",
        );
        let prefix = hit(
            "prefix",
            "ElsewhereFeed",
            NodeKind::FUNCTION,
            0.12,
            "src/components/ElsewhereFeed.tsx",
        );

        assert!(!is_name_resolvable_graph_target("ElsewherePage", &neighbor));
        assert!(is_name_resolvable_graph_target("Elsewhere", &prefix));
    }

    #[test]
    fn route_literal_queries_remain_graph_resolvable() {
        let route = hit(
            "route",
            "GET /api/users (express route; confidence=handler)",
            NodeKind::FUNCTION,
            0.82,
            "src/routes.ts",
        );

        assert!(is_name_resolvable_graph_target("/api/users", &route));
    }

    #[test]
    fn exact_collection_query_prefers_collection_config_over_fields_and_generated_types() {
        let collection = hit(
            "collection",
            "Posts",
            NodeKind::GLOBAL_VARIABLE,
            0.60,
            "src/collections/Posts.ts",
        );
        let generated_field = hit(
            "generated_field",
            "posts",
            NodeKind::FIELD,
            0.95,
            "src/payload-generated-schema.ts",
        );
        let generated_interface = hit(
            "generated_interface",
            "Posts",
            NodeKind::INTERFACE,
            0.95,
            "src/payload-types.ts",
        );
        let script_field = hit(
            "script_field",
            "posts",
            NodeKind::FIELD,
            0.95,
            "scripts/import-wordpress-rich-content.ts",
        );
        let preview_field = hit(
            "preview_field",
            "posts",
            NodeKind::FIELD,
            0.95,
            "src/lib/content-data/preview-content.ts",
        );
        let mut hits = [
            generated_field,
            generated_interface,
            script_field,
            preview_field,
            collection.clone(),
        ];

        hits.sort_by(|left, right| compare_resolution_hits("Posts", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&collection.node_id)
        );
    }

    #[test]
    fn exact_architecture_anchor_prefers_concrete_symbol_over_module_export() {
        let module_export = hit(
            "module_export",
            "SearchService",
            NodeKind::MODULE,
            0.95,
            "crates/codestory-runtime/src/lib.rs",
        );
        let concrete = hit(
            "concrete",
            "codestory_runtime::services::SearchService",
            NodeKind::STRUCT,
            0.60,
            "crates/codestory-runtime/src/search.rs",
        );
        let mut hits = [module_export, concrete.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("SearchService", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&concrete.node_id)
        );
    }

    #[test]
    fn lowercase_collection_query_prefers_collection_config_over_exact_field_case() {
        let collection = hit(
            "collection",
            "Comments",
            NodeKind::GLOBAL_VARIABLE,
            0.60,
            "src/collections/Comments.ts",
        );
        let component_field = hit(
            "component_field",
            "comments",
            NodeKind::FIELD,
            0.95,
            "src/components/PostComments.tsx",
        );
        let mut hits = [component_field, collection.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("comments", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&collection.node_id)
        );
    }

    #[test]
    fn exact_non_primary_query_target_beats_inexact_primary_neighbor() {
        let exact_script = hit(
            "exact_script",
            "seed_payload",
            NodeKind::FUNCTION,
            0.60,
            "scripts/seed-payload.ts",
        );
        let inexact_primary = hit(
            "inexact_primary",
            "seed_payload_preview",
            NodeKind::FUNCTION,
            0.95,
            "src/lib/payload-preview.ts",
        );
        let mut hits = [inexact_primary, exact_script.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("seed_payload", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&exact_script.node_id)
        );
    }

    #[test]
    fn inexact_command_query_prefers_production_entrypoints_over_test_helpers() {
        let production = hit(
            "production",
            "run_index",
            NodeKind::FUNCTION,
            0.60,
            "crates/codestory-cli/src/main.rs",
        );
        let adjacent = hit(
            "adjacent",
            "run_index_once",
            NodeKind::FUNCTION,
            0.80,
            "crates/codestory-cli/src/main.rs",
        );
        let test = hit(
            "test",
            "tests::test_rust_tauri_command_registration_indexes_command_symbol_and_boundary",
            NodeKind::FUNCTION,
            0.95,
            "crates/codestory-indexer/src/lib.rs",
        );
        let mut hits = [test, adjacent, production.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("run_index_command", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );
    }

    #[test]
    fn inexact_resolution_prefers_production_over_non_primary_roles() {
        let production = hit(
            "production",
            "resolve_context_target",
            NodeKind::FUNCTION,
            0.60,
            "crates/codestory-cli/src/runtime.rs",
        );
        let generated = hit(
            "generated",
            "resolve_context_target_generated",
            NodeKind::FUNCTION,
            0.95,
            "target/generated/runtime.rs",
        );
        let docs = hit(
            "docs",
            "resolve_context_target_docs",
            NodeKind::FUNCTION,
            0.95,
            "docs/runtime.md",
        );
        let bench = hit(
            "bench",
            "resolve_context_target_bench",
            NodeKind::FUNCTION,
            0.95,
            "benches/runtime.rs",
        );
        let vendor = hit(
            "vendor",
            "resolve_context_target_vendor",
            NodeKind::FUNCTION,
            0.95,
            "vendor/runtime.rs",
        );

        for non_primary in [generated, docs, bench, vendor] {
            let mut hits = [non_primary, production.clone()];
            hits.sort_by(|left, right| {
                compare_resolution_hits("resolve_context_target", left, right)
            });

            assert_eq!(
                hits.first().map(|hit| &hit.node_id),
                Some(&production.node_id)
            );
        }
    }

    #[test]
    fn exact_facade_method_query_prefers_implementation_file() {
        let implementation = hit(
            "implementation",
            "AppController::snippet_context",
            NodeKind::METHOD,
            0.60,
            "crates/codestory-runtime/src/grounding.rs",
        );
        let browser_facade = hit(
            "browser_facade",
            "ReadOnlyBrowserService::snippet_context",
            NodeKind::METHOD,
            0.95,
            "crates/codestory-runtime/src/browser.rs",
        );
        let service_facade = hit(
            "service_facade",
            "GroundingService::snippet_context",
            NodeKind::METHOD,
            0.90,
            "crates/codestory-runtime/src/services.rs",
        );
        let mut hits = [browser_facade, service_facade, implementation.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("snippet_context", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&implementation.node_id)
        );
    }

    #[test]
    fn exact_type_query_prefers_declaration_over_impl_and_member() {
        let temp = tempdir().expect("create temp dir");
        let source = temp.path().join("lib.rs");
        fs::write(
            &source,
            "pub struct AppController;\nimpl AppController {\n    fn open_project(&self) {}\n}\n",
        )
        .expect("write source");
        let path = source.to_string_lossy();
        let declaration = hit("declaration", "AppController", NodeKind::STRUCT, 1.0, &path);
        let mut implementation = hit(
            "implementation",
            "AppController",
            NodeKind::CLASS,
            1.0,
            &path,
        );
        implementation.line = Some(2);
        let mut member = hit(
            "member",
            "AppController::open_project",
            NodeKind::FUNCTION,
            1.0,
            &path,
        );
        member.line = Some(3);
        let mut hits = [implementation, member, declaration.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("AppController", left, right));

        assert_eq!(hits[0].node_id, declaration.node_id);
    }

    #[test]
    fn callable_query_prefers_multiline_definition_with_assignment() {
        let temp = tempdir().expect("create temp dir");
        let declaration_path = temp.path().join("Indexer.h");
        let implementation_path = temp.path().join("Indexer.cpp");
        fs::write(
            &declaration_path,
            "void doIndex(\n    Command command,\n    State state) override;\n",
        )
        .expect("write declaration");
        fs::write(
            &implementation_path,
            "void Indexer::doIndex(\n    Command command,\n    State state)\n{\n    int status = 0;\n    parse(command, status);\n}\n",
        )
        .expect("write implementation");
        let declaration_path = declaration_path.to_string_lossy();
        let implementation_path = implementation_path.to_string_lossy();
        let declaration = hit(
            "declaration",
            "Indexer::doIndex",
            NodeKind::METHOD,
            1.0,
            &declaration_path,
        );
        let implementation = hit(
            "implementation",
            "Indexer::doIndex",
            NodeKind::FUNCTION,
            1.0,
            &implementation_path,
        );
        let mut hits = [declaration, implementation.clone()];

        hits.sort_by(|left, right| compare_resolution_hits("Indexer::doIndex", left, right));

        assert_eq!(hits[0].node_id, implementation.node_id);
    }
}
