use super::{HashSet, SearchPlanDroppedTermDto, SearchPlanTermsDto};

pub(super) const SEARCH_PLAN_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "and",
    "anchor",
    "answer",
    "are",
    "around",
    "as",
    "at",
    "be",
    "by",
    "can",
    "cite",
    "cited",
    "cites",
    "code",
    "codestory",
    "does",
    "explain",
    "for",
    "from",
    "how",
    "in",
    "into",
    "is",
    "it",
    "later",
    "of",
    "on",
    "or",
    "repo",
    "repository",
    "show",
    "that",
    "the",
    "then",
    "this",
    "through",
    "to",
    "turns",
    "what",
    "where",
    "which",
    "why",
    "with",
];
pub(super) const SEARCH_PLAN_SYMBOL_TERMS: &[&str] = &[
    "indexer",
    "service",
    "storage",
    "store",
    "posts",
    "feed",
    "auth",
    "trail",
    "snippet",
    "workspace",
    "persistence",
    "snapshot",
];
pub(super) const SEARCH_PLAN_OPTIONAL_SUBQUERY_LIMIT: usize = 8;
pub(super) const SEARCH_PLAN_MAX_SEED_ANCHORS: usize = 32;
pub(super) const SEARCH_PLAN_SEED_ANCHOR_MARKER: &str = "Seed anchors:";
pub(super) const SEARCH_PLAN_EXPLICIT_ANCHOR_MARKER: &str = "Anchor the answer around";
pub(super) const SEARCH_PLAN_ROLE_SPECS: &[(&str, &[&str])] = &[
    (
        "indexing_pipeline",
        &["full", "index", "indexing", "indexer", "workspace", "store"],
    ),
    (
        "build_index_entrypoint",
        &["project", "indexing", "build", "index"],
    ),
    (
        "source_group_configuration",
        &[
            "project",
            "source-group",
            "source",
            "group",
            "configuration",
        ],
    ),
    (
        "indexing_work",
        &["indexing", "indexed", "indexer", "command", "work"],
    ),
    (
        "storage_access_surface",
        &[
            "storage",
            "access",
            "accessed",
            "data",
            "application",
            "persistence",
        ],
    ),
    (
        "workspace_discovery",
        &["workspace", "file", "discovery", "source"],
    ),
    (
        "symbol_extraction",
        &["symbol", "extraction", "indexer", "indexing"],
    ),
    (
        "runtime_boundary",
        &["cli", "runtime", "command", "service"],
    ),
    (
        "exec_cli_surface",
        &["exec", "cli", "json", "subcommand", "runtime"],
    ),
    (
        "exec_event_output_surface",
        &[
            "exec",
            "event",
            "events",
            "json",
            "jsonl",
            "output",
            "event processor",
        ],
    ),
    (
        "read_surface",
        &["search", "trail", "snippet", "context", "explore"],
    ),
    (
        "collection_config_surface",
        &[
            "payload",
            "collection",
            "collections",
            "schema",
            "hooks",
            "access",
            "config",
        ],
    ),
    (
        "comment_submission_surface",
        &["comments", "comment", "auth", "submission", "guard"],
    ),
    (
        "public_feed_surface",
        &["feed", "rss", "elsewhere", "social", "entries"],
    ),
    (
        "content_surface",
        &["posts", "comments", "auth", "feed", "elsewhere"],
    ),
    (
        "persistence_surface",
        &[
            "storage",
            "store",
            "persistence",
            "payload",
            "collection",
            "snapshot",
            "refresh",
        ],
    ),
];
pub(super) const SEARCH_PLAN_BASE_SOURCE_TRUTH_CHECKS: &[&str] = &[
    "Draft the CodeStory-only answer from selected anchors, bridge status, symbol, trail, and snippet evidence before opening source.",
    "Open the cited source files after the CodeStory-only draft and classify each claim as correct, partial, misleading, or unsupported.",
];
pub(super) const SEARCH_PLAN_REPO_TEXT_SOURCE_TRUTH_CHECK: &str = "Repo-text-only or ambiguous groups require direct source reads before they can support architecture claims.";

pub(super) fn search_plan_terms(query: &str) -> SearchPlanTermsDto {
    let mut extracted = Vec::new();
    let mut dropped = Vec::new();
    let mut seen = HashSet::new();
    let mut dropped_seen = HashSet::new();

    for raw in query.split_whitespace() {
        let token = raw
            .trim_matches(|ch: char| {
                matches!(
                    ch,
                    '"' | '\'' | '`' | ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']'
                )
            })
            .trim_end_matches("'s");
        if token.is_empty() {
            continue;
        }
        let fragments = token
            .split('/')
            .flat_map(|part| part.split('.'))
            .flat_map(|part| part.split(':'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        for fragment in fragments {
            let normalized = fragment
                .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));
            if normalized.is_empty() {
                continue;
            }
            add_search_plan_term(
                normalized,
                &mut extracted,
                &mut seen,
                &mut dropped,
                &mut dropped_seen,
            );
            if normalized.contains('-') {
                for part in normalized.split('-').filter(|part| !part.is_empty()) {
                    add_search_plan_term(
                        part,
                        &mut extracted,
                        &mut seen,
                        &mut dropped,
                        &mut dropped_seen,
                    );
                }
            }
            for camel_part in split_camel_identifier(normalized) {
                add_search_plan_term(
                    &camel_part,
                    &mut extracted,
                    &mut seen,
                    &mut dropped,
                    &mut dropped_seen,
                );
            }
        }
    }
    drop_search_plan_brand_terms_for_content_flow(query, &mut extracted, &mut dropped);
    add_search_plan_inferred_architecture_terms(
        query,
        &mut extracted,
        &mut seen,
        &mut dropped,
        &mut dropped_seen,
    );

    SearchPlanTermsDto { extracted, dropped }
}

pub(super) fn add_search_plan_inferred_architecture_terms(
    query: &str,
    extracted: &mut Vec<String>,
    seen: &mut HashSet<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
    dropped_seen: &mut HashSet<String>,
) {
    let lower = query.to_ascii_lowercase();
    let has_source_group = lower.contains("source-group")
        || (search_plan_query_has_token(&lower, "source")
            && search_plan_query_has_token(&lower, "group"));
    if has_source_group {
        add_search_plan_term("SourceGroup", extracted, seen, dropped, dropped_seen);
    }

    let has_indexing_work = search_plan_query_has_token(&lower, "indexing")
        && (search_plan_query_has_token(&lower, "work")
            || search_plan_query_has_token(&lower, "command")
            || has_source_group);
    if has_indexing_work {
        add_search_plan_term("build", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("index", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("BuildIndex", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("indexer", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("IndexerCommand", extracted, seen, dropped, dropped_seen);
    }

    let has_data_access = search_plan_query_has_token(&lower, "data")
        && (search_plan_query_has_token(&lower, "access")
            || search_plan_query_has_token(&lower, "accessed"))
        && search_plan_query_has_token(&lower, "application");
    if has_data_access {
        add_search_plan_term("access", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("storage", extracted, seen, dropped, dropped_seen);
        add_search_plan_term("persistence", extracted, seen, dropped, dropped_seen);
    }

    let has_event_output = search_plan_query_has_token(&lower, "event")
        && (search_plan_query_has_token(&lower, "output")
            || search_plan_query_has_token(&lower, "notification")
            || search_plan_query_has_token(&lower, "notifications")
            || search_plan_query_has_token(&lower, "jsonl"));
    if has_event_output {
        add_search_plan_term("EventProcessor", extracted, seen, dropped, dropped_seen);
    }

    if search_plan_query_has_exec_json_flow(&lower) {
        for term in [
            "exec cli",
            "exec runtime",
            "exec session",
            "event processor",
            "event output",
            "thread start",
            "turn start",
        ] {
            add_search_plan_term(term, extracted, seen, dropped, dropped_seen);
        }
    }

    if search_plan_query_has_payload_content_flow(&lower) {
        for term in [
            "content config",
            "collection config",
            "Posts",
            "Comments",
            "social entries",
            "post page",
            "content client",
            "comment submission",
            "comment auth",
            "feed",
        ] {
            add_search_plan_term(term, extracted, seen, dropped, dropped_seen);
        }
    }
}

pub(super) fn search_plan_query_has_exec_json_flow(lower_query: &str) -> bool {
    search_plan_query_has_token(lower_query, "exec")
        && (search_plan_query_has_token(lower_query, "json")
            || search_plan_query_has_token(lower_query, "jsonl"))
        && (search_plan_query_has_token(lower_query, "event")
            || search_plan_query_has_token(lower_query, "events")
            || search_plan_query_has_token(lower_query, "output"))
}

pub(super) fn search_plan_query_has_token(lower_query: &str, token: &str) -> bool {
    lower_query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == token)
}

pub(super) fn search_plan_query_has_payload_content_flow(lower_query: &str) -> bool {
    search_plan_query_has_token(lower_query, "payload")
        && (search_plan_query_has_token(lower_query, "posts")
            || search_plan_query_has_token(lower_query, "post")
            || search_plan_query_has_token(lower_query, "writing"))
        && (search_plan_query_has_token(lower_query, "comments")
            || search_plan_query_has_token(lower_query, "comment")
            || search_plan_query_has_token(lower_query, "feed")
            || search_plan_query_has_token(lower_query, "rss")
            || search_plan_query_has_token(lower_query, "elsewhere")
            || search_plan_query_has_token(lower_query, "social"))
}

pub(super) fn drop_search_plan_brand_terms_for_content_flow(
    query: &str,
    extracted: &mut Vec<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
) {
    let lower = query.to_ascii_lowercase();
    if !(search_plan_query_has_payload_content_flow(&lower)
        && search_plan_query_has_token(&lower, "root")
        && search_plan_query_has_token(&lower, "runtime"))
    {
        return;
    }

    extracted.retain(|term| {
        let is_brand = term.eq_ignore_ascii_case("root") || term.eq_ignore_ascii_case("runtime");
        if is_brand {
            dropped.push(SearchPlanDroppedTermDto {
                term: term.clone(),
                reason: "brand_phrase_in_content_flow".to_string(),
            });
        }
        !is_brand
    });
}

pub(super) fn add_search_plan_term(
    raw: &str,
    extracted: &mut Vec<String>,
    seen: &mut HashSet<String>,
    dropped: &mut Vec<SearchPlanDroppedTermDto>,
    dropped_seen: &mut HashSet<String>,
) {
    let clean = raw
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .to_string();
    if clean.is_empty() {
        return;
    }
    let lower = clean.to_ascii_lowercase();
    if lower.len() < 3 {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "too_short".to_string(),
            });
        }
        return;
    }
    if SEARCH_PLAN_STOPWORDS.contains(&lower.as_str()) {
        if dropped_seen.insert(lower.clone()) {
            dropped.push(SearchPlanDroppedTermDto {
                term: clean,
                reason: "natural_language_filler".to_string(),
            });
        }
        return;
    }
    let value = if clean.chars().any(|ch| ch.is_ascii_uppercase()) && clean.len() > 3 {
        clean
    } else {
        lower.clone()
    };
    if seen.insert(value.to_ascii_lowercase()) {
        extracted.push(value);
    }
}

pub(super) fn split_camel_identifier(value: &str) -> Vec<String> {
    if !value.chars().any(|ch| ch.is_ascii_uppercase()) {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
            continue;
        }
        if ch.is_ascii_uppercase() && !current.is_empty() {
            if current.len() >= 3 {
                parts.push(current.clone());
            }
            current.clear();
        }
        current.push(ch.to_ascii_lowercase());
    }
    if current.len() >= 3 {
        parts.push(current);
    }
    parts
}
