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
    if matches!(term, "source" | "sources") {
        let prompt_terms = prompt_search_terms(question);
        return packet_terms_indicate_buffered_io_flow(&prompt_terms);
    }

    if matches!(term, "bench" | "benchmark" | "benchmarks") {
        let lowered = question.to_ascii_lowercase();
        return lowered.contains("architecture")
            && (lowered.contains("boundary")
                || lowered.contains("boundaries")
                || lowered.contains("across"));
    }

    false
}

fn packet_terms_have_specific_flow_anchor(terms: &[String]) -> bool {
    let has = |term: &str| terms.iter().any(|value| value.eq_ignore_ascii_case(term));
    let has_any = |needles: &[&str]| needles.iter().any(|needle| has(needle));
    (has("extension") && has("host"))
        || ((has("indexing") || has("indexer")) && (has("storage") || has("persistent")))
        || ((has("json") || has("jsonl")) && (has("exec") || has("thread") || has("turn")))
        || packet_terms_indicate_request_dispatch_flow(terms)
        || packet_terms_indicate_server_request_dispatch_flow(terms)
        || packet_terms_indicate_server_route_dispatch_flow(terms)
        || packet_terms_indicate_buffered_io_flow(terms)
        || packet_terms_indicate_site_build_phase_flow(terms)
        || packet_terms_indicate_form_validation_flow(terms)
        || packet_terms_indicate_html_css_template_structure_flow(terms)
        || packet_terms_indicate_shell_install_dispatch_flow(terms)
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
    if (packet_terms_indicate_server_route_dispatch_flow(terms)
        || packet_terms_indicate_server_request_dispatch_flow(terms))
        && !explicit_client_transport
    {
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

pub(crate) fn packet_terms_indicate_server_request_dispatch_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let server_or_protocol = has_any(&["wsgi", "asgi", "server", "servers"]);
    let request_flow = has_any(&["request", "requests"]) && has_any(&["dispatch", "dispatches"]);
    let handler_or_view = has_any(&[
        "handler",
        "handlers",
        "handling",
        "view",
        "views",
        "context",
        "response",
        "responses",
    ]);

    (server_or_protocol && has_any(&["request", "requests"]) && handler_or_view)
        || (request_flow && has_any(&["server", "view", "views", "context", "response"]))
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

pub(crate) fn packet_terms_indicate_javascript_route_source_flow(terms: &[String]) -> bool {
    packet_terms_indicate_server_route_dispatch_flow(terms)
        && packet_terms_have_any(
            terms,
            &[
                "application",
                "applications",
                "express",
                "javascript",
                "js",
                "middleware",
                "response",
                "responses",
                "helper",
                "helpers",
            ],
        )
}

pub(crate) fn packet_terms_indicate_route_tree_dispatch_flow(terms: &[String]) -> bool {
    packet_terms_indicate_server_route_dispatch_flow(terms)
        && packet_terms_have_any(
            terms,
            &[
                "engine",
                "engines",
                "group",
                "groups",
                "method",
                "methods",
                "tree",
                "trees",
                "routergroup",
            ],
        )
}

pub(crate) fn packet_terms_indicate_buffered_io_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    has_any(&["buffer", "buffers", "buffered"])
        && (has_any(&["source", "sources", "sink", "sinks"])
            || (has_any(&["read", "reads", "reader", "write", "writes", "writer"])
                && has_any(&["byte", "bytes", "stream", "streams", "wrapper", "wrappers"])))
}

pub(crate) fn packet_terms_indicate_site_build_phase_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let build_intent = has_any(&[
        "build",
        "builds",
        "building",
        "built",
        "generate",
        "generates",
    ]);
    let site_intent = has_any(&["site", "sites", "static", "page", "pages"]);
    let phase_terms = [
        ["read", "reads", "reader"],
        ["generate", "generates", "generator"],
        ["render", "renders", "renderer"],
        ["write", "writes", "writer"],
        ["phase", "phases", "lifecycle"],
    ];
    let covered_phases = phase_terms
        .iter()
        .filter(|needles| has_any(needles.as_slice()))
        .count();

    build_intent && site_intent && covered_phases >= 2
}

pub(crate) fn packet_terms_indicate_log_record_handler_flow(terms: &[String]) -> bool {
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let logger_or_log_call = has_any(&["log", "logger", "logging", "message", "messages"]);
    let compound_log_record_intent = terms.iter().any(|term| {
        let lower = term.to_ascii_lowercase();
        lower.contains("log") && lower.contains("record")
    });
    let record_intent = has_any(&["record", "records"]) || compound_log_record_intent;
    let handler_intent = has_any(&[
        "handler",
        "handlers",
        "handle",
        "handled",
        "processing",
        "processor",
        "processors",
        "write",
        "writes",
    ]);

    logger_or_log_call && record_intent && handler_intent
}

pub(crate) fn packet_terms_indicate_mapper_configuration_plan_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let mapper_intent = has_any(&["mapper", "mappers", "mapping", "map", "maps"]);
    let configuration_intent = has_any(&[
        "configuration",
        "config",
        "profile",
        "profiles",
        "mappingconfiguration",
    ]);
    let runtime_api_intent = has_any(&[
        "runtime",
        "api",
        "apis",
        "interface",
        "interfaces",
        "entry",
        "entrypoint",
    ]);
    let source_destination_intent =
        has_any(&["source", "sources"]) && has_any(&["destination", "destinations"]);
    let plan_intent = has_any(&[
        "plan",
        "plans",
        "planner",
        "execution",
        "expression",
        "lambda",
        "typemap",
        "typemaps",
        "type",
        "types",
    ]);

    mapper_intent
        && (configuration_intent || plan_intent)
        && (runtime_api_intent || source_destination_intent || plan_intent || has("objects"))
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

pub(crate) fn packet_terms_indicate_html_css_template_structure_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);
    let html_signal = has("html") || has_any(&["markup", "document", "shell"]);
    let css_signal =
        has("css") || has_any(&["style", "styles", "styling", "stylesheet", "stylesheets"]);
    let structure_signal = has_any(&[
        "app",
        "element",
        "elements",
        "interactive",
        "layout",
        "selector",
        "selectors",
        "shared",
        "structure",
        "template",
        "templates",
        "theme",
    ]);

    html_signal && css_signal && structure_signal
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
    let explicit_client_or_http_intent =
        packet_terms_have_any(terms, &["client", "clients", "http", "httpclient"]);
    let request_intent = packet_terms_have_any(terms, &["request", "requests"]);
    let send_or_transport_intent = packet_terms_have_any(
        terms,
        &["send", "sending", "sent", "transport", "transports"],
    );
    let convenience_or_helper_intent =
        packet_terms_have_any(terms, &["convenience", "helper", "helpers"]);

    (explicit_client_or_http_intent && (send_or_transport_intent || convenience_or_helper_intent))
        || (request_intent && send_or_transport_intent)
}

pub(crate) fn packet_terms_indicate_form_validation_flow(terms: &[String]) -> bool {
    let has = |term: &str| packet_terms_have(terms, term);
    let has_any = |needles: &[&str]| packet_terms_have_any(terms, needles);

    has_any(&["validation", "validate", "validates", "validity", "invalid"])
        && has_any(&["form", "forms", "input", "inputs", "html"])
        && (has_any(&["constraint", "constraints", "native"])
            || has_any(&["custom", "javascript", "script", "scripts", "js"])
            || has("pattern")
            || has_any(&["required", "min", "max"]))
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
    let explicit_url_session_lifecycle = packet_terms_have_any(
        terms,
        &[
            "callback",
            "callbacks",
            "resume",
            "resumes",
            "swift",
            "task",
            "tasks",
            "urlsession",
        ],
    );
    explicit_url_session_lifecycle
        && packet_terms_have_any(terms, &["session", "urlsession"])
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

pub(crate) fn packet_terms_indicate_shell_install_dispatch_flow(terms: &[String]) -> bool {
    packet_terms_have_any(
        terms,
        &["bash", "shell", "script", "function", "command", "commands"],
    ) && packet_terms_have_any(
        terms,
        &[
            "install",
            "installer",
            "bootstraps",
            "bootstrap",
            "download",
            "downloads",
            "completion",
            "profile",
            "source",
            "sourced",
            "use",
        ],
    ) && packet_terms_have_any(terms, &["dispatch", "dispatches", "function", "commands"])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffered_io_prompts_retain_source_as_api_concept() {
        let terms = packet_probe_terms(
            "Explain how buffered Source and Sink wrappers use Buffer state during reads and writes.",
        );
        for expected in ["source", "sink", "buffered", "buffer"] {
            assert!(
                terms.iter().any(|term| term == expected),
                "expected {expected:?} in {terms:?}"
            );
        }
        assert!(packet_terms_indicate_buffered_io_flow(&terms));
    }

    #[test]
    fn site_build_prompts_are_detected_from_lifecycle_terms() {
        let terms = packet_probe_terms(
            "Trace how the build command creates a site and runs the read, generate, render, and write phases.",
        );
        assert!(packet_terms_indicate_site_build_phase_flow(&terms));
    }

    #[test]
    fn mapper_configuration_plan_prompts_are_detected_from_flow_terms() {
        let terms = packet_probe_terms(
            "Explain how mapper configuration and runtime mapper APIs cooperate to map source objects to destination objects through type map plans.",
        );

        assert!(packet_terms_indicate_mapper_configuration_plan_flow(&terms));
    }

    #[test]
    fn client_send_prompts_require_client_or_send_intent() {
        let client_terms = packet_probe_terms(
            "Explain how an HTTP package exposes top-level helpers, Client convenience methods, BaseRequest finalization, and IOClient send behavior.",
        );
        assert!(packet_terms_indicate_client_send_flow(&client_terms));

        let route_terms = packet_probe_terms(
            "Trace how Express creates an application, registers middleware routes, and handles an incoming request through the router and response helpers.",
        );
        assert!(!packet_terms_indicate_client_send_flow(&route_terms));
    }

    #[test]
    fn url_session_prompts_require_explicit_lifecycle_terms() {
        let swift_terms = packet_probe_terms(
            "Trace how a Session creates requests, resumes tasks, validates data requests, and receives URLSession callbacks.",
        );
        assert!(packet_terms_indicate_url_session_request_flow(&swift_terms));

        let python_terms = packet_probe_terms(
            "Explain how Requests turns a top-level request call into a prepared request and sends it through a session adapter.",
        );
        assert!(!packet_terms_indicate_url_session_request_flow(
            &python_terms
        ));
    }

    #[test]
    fn route_tree_prompts_are_distinct_from_javascript_route_sources() {
        let express_terms = packet_probe_terms(
            "Trace how Express creates an application, registers middleware/routes, and handles an incoming request through the router and response helpers.",
        );
        assert!(packet_terms_indicate_server_route_dispatch_flow(
            &express_terms
        ));
        assert!(packet_terms_indicate_javascript_route_source_flow(
            &express_terms
        ));
        assert!(!packet_terms_indicate_route_tree_dispatch_flow(
            &express_terms
        ));

        let gin_terms = packet_probe_terms(
            "Trace how Gin creates an engine, registers routes through router groups, stores them in method trees, and dispatches handlers for a request.",
        );
        assert!(packet_terms_indicate_server_route_dispatch_flow(&gin_terms));
        assert!(packet_terms_indicate_route_tree_dispatch_flow(&gin_terms));
    }

    #[test]
    fn server_request_dispatch_prompts_do_not_activate_client_transport() {
        let terms = packet_probe_terms(
            "Trace how a WSGI app receives a request, opens request handling, dispatches to a view, finalizes the response, and returns control to the server.",
        );

        assert!(packet_terms_indicate_server_request_dispatch_flow(&terms));
        assert!(!packet_terms_indicate_request_dispatch_flow(&terms));
    }

    #[test]
    fn html_css_template_structure_prompts_are_detected() {
        let terms = packet_probe_terms(
            "Explain how an HTML app shell and CSS structure split template selectors, theme defaults, and interactive element styling.",
        );

        assert!(packet_terms_indicate_html_css_template_structure_flow(
            &terms
        ));
    }

    #[test]
    fn form_validation_prompts_are_detected_from_constraint_and_custom_terms() {
        let terms = packet_probe_terms(
            "Explain how form validation examples combine native HTML constraints with custom JavaScript validation.",
        );
        assert!(packet_terms_indicate_form_validation_flow(&terms));
    }

    #[test]
    fn shell_install_dispatch_prompts_are_detected_from_bootstrap_and_dispatch_terms() {
        let terms = packet_probe_terms(
            "Trace how an install script bootstraps the shell function and dispatches install, download, and use commands.",
        );
        assert!(packet_terms_indicate_shell_install_dispatch_flow(&terms));
    }
}
