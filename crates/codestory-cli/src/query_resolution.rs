use anyhow::{Context, Result};
use codestory_contracts::api::{NodeKind, SearchHit, SearchMatchQualityDto};
use codestory_runtime::{compare_ranked_hits, symbol_name_match_rank};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ResolutionRank {
    source_truth_bucket: u8,
    exact_display: u8,
    collection_definition_path: u8,
    exact_case_match: u8,
    exact_terminal: u8,
    inexact_query_prefix_match: u8,
    implementation_path: u8,
    type_definition_line: u8,
    callable_definition_line: u8,
    declaration_anchor: u8,
    kind_bucket: u8,
    exact_leading: u8,
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
        source_truth_bucket: source_truth_bucket(hit),
        exact_display: rank.exact_display,
        collection_definition_path: collection_definition_path_bucket(query, hit),
        exact_case_match: exact_case_match_bucket(query, hit),
        exact_terminal: rank.exact_terminal,
        inexact_query_prefix_match: inexact_query_prefix_match_bucket(query, hit),
        implementation_path: implementation_path_bucket(hit),
        type_definition_line: type_definition_line_bucket(project_root, query, hit),
        callable_definition_line: callable_definition_line_bucket(project_root, query, hit),
        declaration_anchor: declaration_anchor_bucket(hit),
        kind_bucket: resolution_kind_bucket(hit.kind),
        exact_leading: rank.exact_leading,
    }
}

pub(crate) fn is_graph_target_candidate(hit: &SearchHit) -> bool {
    !matches!(
        hit.match_quality,
        Some(SearchMatchQualityDto::SemanticSuggestion | SearchMatchQualityDto::RepoText)
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

    let query = codestory_runtime::normalize_symbol_query(query);
    if query.is_empty() {
        return false;
    }
    let display = codestory_runtime::normalize_symbol_query(&hit.display_name);
    if query.contains('/') && display.contains(&query) {
        return true;
    }
    let terminal = codestory_runtime::terminal_symbol_segment(&hit.display_name);
    let leading = codestory_runtime::leading_symbol_segment(&hit.display_name);
    display.starts_with(&query) || terminal.starts_with(&query) || leading.starts_with(&query)
}

fn inexact_query_prefix_match_bucket(query: &str, hit: &SearchHit) -> u8 {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    if rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0 {
        return 0;
    }
    let query = codestory_runtime::normalize_symbol_query(query);
    let terminal = codestory_runtime::terminal_symbol_segment(&hit.display_name);
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
    if hit.display_name.starts_with("tests::") {
        return true;
    }

    hit.file_path
        .as_deref()
        .map(|path| {
            let path = format!("/{}", normalize_path_fragment(path));
            let non_primary_marker = [
                "/bin/test/",
                "/test/data/",
                "/tests/",
                "/fixtures/",
                "/fixture/",
                "/examples/",
                "/example/",
                "/src/external/",
                "/external/",
                "/vendor/",
                "/vendors/",
                "/third_party/",
                "/third-party/",
                "/scripts/",
            ]
            .iter()
            .any(|marker| path.contains(marker));
            let generated_marker = path.contains("generated")
                || path.contains("payload-types")
                || path.contains("/target/")
                || path.contains("/dist/")
                || path.contains("/build/");

            non_primary_marker || generated_marker
        })
        .unwrap_or(false)
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
    let relative = normalize_path_fragment(&crate::display::relative_path(project_root, file_path));
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
    crate::display::clean_path_string(value).to_ascii_lowercase()
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
    let expected = codestory_runtime::terminal_symbol_segment(query);
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
    let expected = codestory_runtime::terminal_symbol_segment(query);
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{NodeId, SearchHitOrigin};

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
}
