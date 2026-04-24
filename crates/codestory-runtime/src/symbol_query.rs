use codestory_contracts::api::{NodeKind, SearchHit, SearchHitOrigin};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolNameMatchRank {
    pub exact_display: u8,
    pub exact_terminal: u8,
    pub exact_leading: u8,
}

pub fn normalize_symbol_query(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub fn terminal_symbol_segment(value: &str) -> String {
    value
        .rsplit([':', '.', '/', '\\'])
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn leading_symbol_segment(value: &str) -> String {
    value
        .split("::")
        .next()
        .map(normalize_symbol_query)
        .unwrap_or_default()
}

pub fn symbol_name_match_rank(query: &str, display_name: &str) -> SymbolNameMatchRank {
    let query = normalize_symbol_query(query);
    let display = normalize_symbol_query(display_name);
    let terminal = terminal_symbol_segment(display_name);
    let leading = leading_symbol_segment(display_name);

    SymbolNameMatchRank {
        exact_display: u8::from(display == query),
        exact_terminal: u8::from(terminal == query),
        exact_leading: u8::from(leading == query),
    }
}

pub fn compare_ranked_hits<T: Ord>(
    left: &SearchHit,
    right: &SearchHit,
    left_rank: T,
    right_rank: T,
) -> Ordering {
    right_rank
        .cmp(&left_rank)
        .then_with(|| right.score.total_cmp(&left.score))
        .then_with(|| left.display_name.len().cmp(&right.display_name.len()))
        .then_with(|| left.display_name.cmp(&right.display_name))
}

fn search_kind_bucket(kind: NodeKind, origin: SearchHitOrigin) -> u8 {
    if origin == SearchHitOrigin::TextMatch {
        return 0;
    }

    match kind {
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 3,
        NodeKind::FUNCTION
        | NodeKind::METHOD
        | NodeKind::MACRO
        | NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 2,
        NodeKind::UNKNOWN => 0,
        _ => 1,
    }
}

fn search_kind_tiebreak(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::FUNCTION => 4,
        NodeKind::METHOD => 3,
        NodeKind::MACRO => 2,
        NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 1,
        _ => 0,
    }
}

fn inexact_search_kind_bucket(kind: NodeKind, origin: SearchHitOrigin) -> u8 {
    if origin == SearchHitOrigin::TextMatch {
        return 0;
    }

    match kind {
        NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => 3,
        NodeKind::FIELD
        | NodeKind::VARIABLE
        | NodeKind::GLOBAL_VARIABLE
        | NodeKind::CONSTANT
        | NodeKind::ENUM_CONSTANT => 2,
        NodeKind::MODULE
        | NodeKind::NAMESPACE
        | NodeKind::PACKAGE
        | NodeKind::STRUCT
        | NodeKind::CLASS
        | NodeKind::INTERFACE
        | NodeKind::ENUM
        | NodeKind::UNION
        | NodeKind::TYPEDEF => 1,
        NodeKind::UNKNOWN => 0,
        _ => 1,
    }
}

fn is_type_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::STRUCT
            | NodeKind::CLASS
            | NodeKind::INTERFACE
            | NodeKind::ENUM
            | NodeKind::UNION
            | NodeKind::TYPEDEF
            | NodeKind::TYPE_PARAMETER
    )
}

fn query_mentions_type_role(query: &str) -> bool {
    let mut previous_was_data = false;
    for term in query.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_') {
        let term = term.to_ascii_lowercase();
        if matches!(
            term.as_str(),
            "struct" | "record" | "class" | "interface" | "enum" | "type" | "typedef"
        ) || (previous_was_data && term == "type")
        {
            return true;
        }
        previous_was_data = term == "data";
    }
    false
}

fn query_kind_intent_bucket(query: &str, kind: NodeKind, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    u8::from(query_mentions_type_role(query) && is_type_like_kind(kind))
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter_map(|term| {
            let normalized = term.trim().to_ascii_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

fn terms_contain_phrase(terms: &[String], phrase: &[&str]) -> bool {
    terms
        .windows(phrase.len())
        .any(|window| window.iter().map(String::as_str).eq(phrase.iter().copied()))
}

fn query_entrypoint_intent_bucket(query: &str, display_name: &str, is_exact_match: bool) -> u8 {
    if is_exact_match {
        return 0;
    }
    let terms = query_terms(query);
    let terminal = terminal_symbol_segment(display_name);

    u8::from(
        (terminal == "node_details" && terms_contain_phrase(&terms, &["node", "details"]))
            || (terminal == "source_files" && terms_contain_phrase(&terms, &["source", "files"]))
            || (terminal == "compare_resolution_hits"
                && terms_contain_phrase(&terms, &["compare", "resolution", "hits"]))
            || (terminal == "file_text_match_line"
                && terms_contain_phrase(&terms, &["file", "text", "match", "line"]))
            || (matches!(terminal.as_str(), "parse" | "llamacpp_embeddings_url_env")
                && terms_contain_phrase(&terms, &["endpoint"])),
    )
}

pub(crate) fn query_mentions_non_primary_source(query: &str) -> bool {
    let terms = query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();

    terms.iter().enumerate().any(|(index, term)| {
        is_non_primary_source_term(term) && !is_non_primary_source_exclusion_context(&terms, index)
    })
}

fn is_non_primary_source_term(term: &str) -> bool {
    matches!(
        term,
        "test"
            | "tests"
            | "testing"
            | "example"
            | "examples"
            | "sample"
            | "samples"
            | "script"
            | "scripts"
            | "bench"
            | "benchmark"
            | "benchmarks"
            | "fixture"
            | "fixtures"
            | "external"
            | "vendor"
            | "vendors"
            | "vendored"
            | "thirdparty"
            | "third_party"
            | "third-party"
    )
}

fn is_non_primary_source_exclusion_context(terms: &[String], index: usize) -> bool {
    let start = index.saturating_sub(3);
    terms[start..index].iter().any(|term| {
        matches!(
            term.as_str(),
            "avoid"
                | "demote"
                | "exclude"
                | "excluding"
                | "hide"
                | "ignore"
                | "omit"
                | "skip"
                | "without"
        )
    })
}

pub(crate) fn is_non_primary_source_hit(hit: &SearchHit) -> bool {
    if hit.display_name.starts_with("tests::") {
        return true;
    }

    hit.file_path
        .as_deref()
        .map(|path| {
            let path = format!("/{}", path.replace('\\', "/").to_ascii_lowercase());
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
            ]
            .iter()
            .any(|marker| path.contains(marker));
            let script_benchmark_harness = path.contains("/scripts/")
                && (path.contains("bench") || path.contains("benchmark"));

            non_primary_marker || script_benchmark_harness
        })
        .unwrap_or(false)
}

fn search_match_rank(query: &str, hit: &SearchHit) -> (u8, u8, u8, u8, u8, u8, u8, u8, u8) {
    let rank = symbol_name_match_rank(query, &hit.display_name);
    let is_exact_match =
        rank.exact_display != 0 || rank.exact_terminal != 0 || rank.exact_leading != 0;
    let source_bucket = u8::from(
        is_exact_match
            || query_mentions_non_primary_source(query)
            || !is_non_primary_source_hit(hit),
    );
    let kind_bucket = if is_exact_match {
        search_kind_bucket(hit.kind, hit.origin)
    } else {
        inexact_search_kind_bucket(hit.kind, hit.origin)
    };
    let query_kind_intent = query_kind_intent_bucket(query, hit.kind, is_exact_match);
    let query_entrypoint_intent =
        query_entrypoint_intent_bucket(query, &hit.display_name, is_exact_match);
    let kind_tiebreak = if is_exact_match {
        search_kind_tiebreak(hit.kind)
    } else {
        0
    };

    (
        rank.exact_display,
        rank.exact_terminal,
        source_bucket,
        query_kind_intent,
        query_entrypoint_intent,
        kind_bucket,
        rank.exact_leading,
        kind_tiebreak,
        u8::from(hit.origin == SearchHitOrigin::IndexedSymbol),
    )
}

pub(crate) fn compare_search_hits(query: &str, left: &SearchHit, right: &SearchHit) -> Ordering {
    compare_ranked_hits(
        left,
        right,
        search_match_rank(query, left),
        search_match_rank(query, right),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::NodeId;

    fn hit(id: &str, display_name: &str, kind: NodeKind, score: f32) -> SearchHit {
        SearchHit {
            node_id: NodeId(id.to_string()),
            display_name: display_name.to_string(),
            kind,
            file_path: None,
            line: None,
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            score_breakdown: None,
        }
    }

    fn hit_at_path(
        id: &str,
        display_name: &str,
        kind: NodeKind,
        score: f32,
        path: &str,
    ) -> SearchHit {
        let mut hit = hit(id, display_name, kind, score);
        hit.file_path = Some(path.to_string());
        hit
    }

    #[test]
    fn inexact_queries_use_score_between_callables() {
        let lower_scored_function = hit("lower", "plain_function", NodeKind::FUNCTION, 0.40);
        let higher_scored_method = hit("higher", "Owner::strong_method", NodeKind::METHOD, 0.80);

        let mut hits = [lower_scored_function, higher_scored_method.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&higher_scored_method.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_callables_over_data_members() {
        let callable = hit("callable", "plain_function", NodeKind::FUNCTION, 0.40);
        let field = hit("field", "Owner::strong_field", NodeKind::FIELD, 0.95);

        let mut hits = [field, callable.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&callable.node_id)
        );
    }

    #[test]
    fn inexact_type_role_queries_prefer_type_symbols_over_callables() {
        let refresh_plan = hit("type", "RefreshPlan", NodeKind::STRUCT, 0.40);
        let helper = hit(
            "helper",
            "WorkspaceDiscovery::build_refresh_plan",
            NodeKind::METHOD,
            0.95,
        );

        let mut hits = [helper, refresh_plan.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "struct record data type refresh plan workspace indexing",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&refresh_plan.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_named_node_details_entrypoint() {
        let node_details = hit(
            "node_details",
            "GroundingService::node_details",
            NodeKind::METHOD,
            0.40,
        );
        let edge_digest = hit(
            "edge_digest",
            "edge_digest_for_node",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [edge_digest, node_details.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("node details source occurrence edge digest", left, right)
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&node_details.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_named_source_files_entrypoint() {
        let source_files = hit(
            "source_files",
            "WorkspaceDiscovery::source_files",
            NodeKind::METHOD,
            0.40,
        );
        let language_filter = hit(
            "filter",
            "WorkspaceManifest::should_filter_source_group_language",
            NodeKind::METHOD,
            0.95,
        );

        let mut hits = [language_filter, source_files.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "workspace source files apply language filters and excludes",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&source_files.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_llamacpp_endpoint_parser_entrypoint() {
        let endpoint_parser = hit(
            "parser",
            "LlamaCppEndpoint::parse",
            NodeKind::FUNCTION,
            0.40,
        );
        let url_constant = hit(
            "url-env",
            "LLAMACPP_EMBEDDINGS_URL_ENV",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [url_constant, endpoint_parser.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "llama.cpp embeddings endpoint URL environment variable",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&endpoint_parser.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_compare_resolution_hits_entrypoint() {
        let resolution_hits = hit(
            "resolution_hits",
            "compare_resolution_hits",
            NodeKind::FUNCTION,
            0.40,
        );
        let candidates = hit(
            "candidates",
            "compare_resolution_candidates",
            NodeKind::FUNCTION,
            0.95,
        );

        let mut hits = [candidates, resolution_hits.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "compare resolution hits exact symbol before ambiguous candidates",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&resolution_hits.node_id)
        );
    }

    #[test]
    fn inexact_queries_prefer_file_text_match_line_entrypoint() {
        let file_text_match_line = hit(
            "file_text_match_line",
            "file_text_match_line",
            NodeKind::FUNCTION,
            0.40,
        );
        let excerpt = hit("excerpt", "repo_text_excerpt", NodeKind::FUNCTION, 0.95);

        let mut hits = [excerpt, file_text_match_line.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "file text match line for repo text search terms",
                left,
                right,
            )
        });

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&file_text_match_line.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_tests_unless_requested() {
        let production = hit("production", "plain_function", NodeKind::FUNCTION, 0.40);
        let mut test_hit = hit("test", "tests::strong_case", NodeKind::FUNCTION, 0.95);
        test_hit.file_path = Some("src/module.rs".to_string());

        let mut hits = [test_hit.clone(), production.clone()];
        hits.sort_by(|left, right| compare_search_hits("describe strong behavior", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| compare_search_hits("test strong behavior", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&test_hit.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("describe behavior that should hide tests", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_external_sources_unless_requested() {
        let production = hit_at_path(
            "production",
            "SqliteIndexStorage::addNode",
            NodeKind::FUNCTION,
            0.40,
            "src/lib/data/storage/sqlite/SqliteIndexStorage.cpp",
        );
        let external = hit_at_path(
            "external",
            "sqlite3SrcListIndexedBy",
            NodeKind::FUNCTION,
            0.95,
            "src/external/sqlite/sqlite3.c",
        );

        let mut hits = [external.clone(), production.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits(
                "index storage should find project storage code",
                left,
                right,
            )
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("external sqlite API indexed source list", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&external.node_id)
        );
    }

    #[test]
    fn inexact_queries_downrank_script_benchmarks_unless_requested() {
        let production = hit_at_path(
            "production",
            "handle_http_request",
            NodeKind::FUNCTION,
            0.40,
            "crates/codestory-cli/src/main.rs",
        );
        let benchmark_script = hit_at_path(
            "benchmark",
            "waitForHttpHealth",
            NodeKind::FUNCTION,
            0.95,
            "scripts/cross-repo-promotion-benchmark.mjs",
        );
        let application_script = hit_at_path(
            "script",
            "sendWalletBatch",
            NodeKind::FUNCTION,
            0.95,
            "scripts/hunter.js",
        );

        let mut hits = [benchmark_script.clone(), production.clone()];
        hits.sort_by(|left, right| {
            compare_search_hits("route small http server requests", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&production.node_id)
        );

        hits.sort_by(|left, right| {
            compare_search_hits("benchmark waits for http server health", left, right)
        });
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&benchmark_script.node_id)
        );

        let mut hits = [application_script.clone(), production.clone()];
        hits.sort_by(|left, right| compare_search_hits("send wallet batch calls", left, right));
        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&application_script.node_id)
        );
    }

    #[test]
    fn exact_non_primary_symbol_matches_are_not_downranked() {
        let production = hit_at_path(
            "production",
            "Project::Parse",
            NodeKind::FUNCTION,
            0.95,
            "src/lib/project/Project.cpp",
        );
        let external = hit_at_path(
            "external",
            "TiXmlDocument::Parse",
            NodeKind::FUNCTION,
            0.40,
            "src/external/tinyxml/tinyxml.cpp",
        );

        let mut hits = [production, external.clone()];
        hits.sort_by(|left, right| compare_search_hits("TiXmlDocument::Parse", left, right));

        assert_eq!(
            hits.first().map(|hit| &hit.node_id),
            Some(&external.node_id)
        );
    }
}
