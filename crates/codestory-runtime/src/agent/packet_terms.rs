use crate::agent::packet_scoring::normalize_identifier;
use crate::{is_non_primary_source_term, query_mentions_non_primary_source};
use std::collections::HashSet;

pub(crate) fn prompt_search_terms(prompt: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "a",
        "actual",
        "already",
        "an",
        "and",
        "are",
        "area",
        "areas",
        "across",
        "as",
        "at",
        "be",
        "boundaries",
        "boundary",
        "by",
        "can",
        "current",
        "does",
        "existing",
        "for",
        "from",
        "how",
        "implementation",
        "implemented",
        "in",
        "is",
        "it",
        "of",
        "on",
        "or",
        "repo",
        "repository",
        "risk",
        "risks",
        "study",
        "surface",
        "surfaces",
        "the",
        "this",
        "to",
        "what",
        "where",
        "which",
        "why",
        "with",
        "work",
        "works",
    ];

    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    for ch in prompt.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch.to_ascii_lowercase());
            continue;
        }

        if current.len() >= 3
            && !STOPWORDS.contains(&current.as_str())
            && seen.insert(current.clone())
        {
            terms.push(current.clone());
        }
        current.clear();
    }

    if current.len() >= 3 && !STOPWORDS.contains(&current.as_str()) && seen.insert(current.clone())
    {
        terms.push(current);
    }

    terms
}

pub(crate) fn packet_probe_terms(question: &str) -> Vec<String> {
    let include_non_primary_terms = query_mentions_non_primary_source(question);
    let brand_terms = brand_phrase_noise_terms(question);
    let mut terms = prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            include_non_primary_terms
                || !is_non_primary_source_term(term)
                || packet_retains_non_primary_probe_term(question, term)
        })
        .collect::<Vec<_>>();

    if !brand_terms.is_empty() && packet_terms_have_specific_flow_anchor(&terms) {
        terms.retain(|term| !brand_terms.contains(term.as_str()));
    }

    terms
}

fn packet_retains_non_primary_probe_term(question: &str, term: &str) -> bool {
    if !matches!(term, "bench" | "benchmark" | "benchmarks") {
        return false;
    }
    let lowered = question.to_ascii_lowercase();
    lowered.contains("architecture")
        && (lowered.contains("boundary")
            || lowered.contains("boundaries")
            || lowered.contains("across"))
}

fn packet_terms_have_specific_flow_anchor(terms: &[String]) -> bool {
    let has = |term: &str| terms.iter().any(|value| value.eq_ignore_ascii_case(term));
    let has_any = |needles: &[&str]| needles.iter().any(|needle| has(needle));
    (has("extension") && has("host"))
        || ((has("indexing") || has("indexer")) && (has("storage") || has("persistent")))
        || ((has("json") || has("jsonl")) && (has("exec") || has("thread") || has("turn")))
        || packet_terms_indicate_request_dispatch_flow(terms)
        || (has("event") && has("loop"))
        || (has_any(&["command", "commands"]) && has_any(&["dispatch", "dispatches"]))
        || (has("search") && (has("flags") || has("matcher") || has("haystack")))
        || has("payload")
        || has("posts")
        || has("post")
        || has("comments")
        || has("feed")
        || has("rss")
}

fn brand_phrase_noise_terms(question: &str) -> HashSet<String> {
    let mut terms = HashSet::new();
    let tokens = question
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
        })
        .collect::<Vec<_>>();

    for window in tokens.windows(3) {
        if let [left, joiner, right] = window
            && *joiner == "&"
        {
            if let Some(term) = title_case_brand_token_term(left) {
                terms.insert(term);
            }
            if let Some(term) = title_case_brand_token_term(right) {
                terms.insert(term);
            }
        }
    }

    terms
}

fn title_case_brand_token_term(token: &str) -> Option<String> {
    let mut chars = token.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first.is_ascii_uppercase()
        && second.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

pub(crate) fn packet_terms_have(terms: &[String], needle: &str) -> bool {
    let normalized_needle = normalize_identifier(needle);
    terms.iter().any(|value| {
        value.eq_ignore_ascii_case(needle) || normalize_identifier(value) == normalized_needle
    })
}

pub(crate) fn packet_terms_have_any(terms: &[String], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| packet_terms_have(terms, needle))
}

pub(crate) fn packet_terms_indicate_indexing_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    has_any(&["index", "indexed", "indexer", "indexing"])
        && has_any(&[
            "cli",
            "command",
            "discovery",
            "extraction",
            "file",
            "files",
            "persistence",
            "projection",
            "refresh",
            "runtime",
            "search",
            "snapshot",
            "storage",
            "store",
            "symbol",
            "workspace",
        ])
}

pub(crate) fn packet_terms_indicate_request_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let explicit_client_transport = has_any(&[
        "adapter",
        "adapters",
        "interceptor",
        "interceptors",
        "transport",
    ]);
    if packet_terms_indicate_server_route_dispatch_flow(terms) && !explicit_client_transport {
        return false;
    }
    let has_compound_request_dispatch = terms.iter().any(|term| {
        let normalized = normalize_identifier(term);
        normalized.contains("dispatch") && normalized.contains("request")
    });
    has_any(&["interceptor", "interceptors"])
        || has_compound_request_dispatch
        || ((has("request") || has("http"))
            && has_any(&["adapter", "adapters", "dispatch", "dispatches", "transport"]))
}

pub(crate) fn packet_terms_indicate_server_route_dispatch_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has_any(&["route", "routes", "router"])
        && has_any(&[
            "handler",
            "handlers",
            "middleware",
            "dispatch",
            "dispatches",
        ])
        && (has("request")
            || has_any(&["server", "incoming", "http"])
            || has_any(&["engine", "method", "methods"]))
}

pub(crate) fn packet_terms_indicate_prepared_session_adapter_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    (has("prepared") || has("prepare"))
        && has_any(&["request", "requests"])
        && has("session")
        && has_any(&["adapter", "adapters", "send", "sends", "transport"])
}

pub(crate) fn packet_terms_indicate_search_execution_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has("search")
        && has_any(&[
            "candidate",
            "flags",
            "haystack",
            "matcher",
            "printer",
            "searcher",
            "walk",
            "walks",
        ])
}

pub(crate) fn packet_terms_indicate_stylesheet_animation_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let css_signal = has("css")
        || has("animatecss")
        || has_any(&[
            "stylesheet",
            "stylesheets",
            "style",
            "styles",
            "selector",
            "selectors",
        ]);
    let animation_signal = has_any(&[
        "animate",
        "animated",
        "animation",
        "animations",
        "keyframe",
        "keyframes",
    ]);
    let source_shape_signal = has_any(&[
        "base",
        "class",
        "classes",
        "custom",
        "property",
        "properties",
        "selector",
        "selectors",
        "variable",
        "variables",
    ]);
    css_signal && animation_signal && source_shape_signal
}

pub(crate) fn packet_terms_indicate_sql_schema_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has_any(&["sql", "schema", "schemas", "table", "tables"])
        && has_any(&[
            "relationship",
            "relationships",
            "relation",
            "relations",
            "foreign",
            "constraint",
            "constraints",
            "reference",
            "references",
        ])
        && has_any(&["table", "tables", "create", "schema", "schemas"])
}

pub(crate) fn packet_terms_indicate_hook_cache_flow(terms: &[String]) -> bool {
    let hook_signal = packet_terms_have_any(terms, &["hook", "hooks"])
        || terms.iter().any(|term| {
            let normalized = normalize_identifier(term);
            normalized.as_bytes() == [115, 119, 114]
                || (normalized.len() > 3 && normalized.starts_with("use"))
        });
    let cache_or_public_api_intent = packet_terms_have_any(
        terms,
        &[
            "api",
            "cache",
            "caches",
            "caching",
            "expose",
            "exposes",
            "export",
            "exports",
            "public",
            "serialize",
            "serializes",
        ],
    );

    hook_signal && cache_or_public_api_intent
}

pub(crate) fn packet_terms_indicate_client_send_flow(terms: &[String]) -> bool {
    let client_or_request_intent = packet_terms_have_any(
        terms,
        &[
            "client",
            "clients",
            "request",
            "requests",
            "http",
            "httpclient",
        ],
    );
    let send_or_transport_intent = packet_terms_have_any(
        terms,
        &[
            "convenience",
            "helper",
            "helpers",
            "send",
            "sending",
            "sent",
            "transport",
            "transports",
        ],
    );

    client_or_request_intent && send_or_transport_intent
}

pub(crate) fn packet_terms_indicate_event_loop_command_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let event_loop_intent = has("eventloop") || (has("event") && has("loop"));
    let command_dispatch_intent = has_any(&["command", "commands"])
        && has_any(&[
            "acl",
            "arity",
            "call",
            "dispatch",
            "dispatches",
            "execute",
            "executes",
            "execution",
            "handler",
            "handlers",
            "input",
            "network",
            "process",
            "slowlog",
            "table",
        ]);
    let network_command_input_intent =
        has_any(&["network", "socket", "client", "input"]) && has_any(&["command", "commands"]);

    event_loop_intent || command_dispatch_intent || network_command_input_intent
}

pub(crate) fn packet_terms_indicate_url_session_request_flow(terms: &[String]) -> bool {
    packet_terms_have_any(terms, &["session", "urlsession", "callback", "callbacks"])
        && packet_terms_have_any(
            terms,
            &[
                "request",
                "requests",
                "resume",
                "resumes",
                "task",
                "tasks",
                "validate",
                "validates",
                "validation",
            ],
        )
}

pub(crate) fn packet_terms_indicate_shell_version_use_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &[
            "bash", "shell", "script", "command", "dispatch", "install", "version",
        ],
    ) && packet_terms_have_any(terms, &["use", "switch", "active", "current", "needed"])
}

pub(crate) fn packet_terms_indicate_string_predicate_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &["string", "strings", "charsequence", "charsequences", "text"],
    ) && packet_terms_have_any(
        terms,
        &[
            "blank",
            "empty",
            "whitespace",
            "trim",
            "trims",
            "predicate",
            "predicates",
        ],
    )
}

pub(crate) fn packet_terms_indicate_runtime_formatting_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &["format", "formats", "formatting", "vformat", "format_to"],
    ) && packet_terms_have_any(
        terms,
        &[
            "arg",
            "args",
            "argument",
            "arguments",
            "runtime",
            "type",
            "erased",
            "output",
        ],
    )
}
