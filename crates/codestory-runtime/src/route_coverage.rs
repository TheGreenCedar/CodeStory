use super::{
    FrameworkRouteCoverageDto, GraphEdge, GraphNode, RouteEndpointKindDto,
    RouteEndpointMetadataDto, language_support_profile_for_language_name,
};
use serde::Deserialize;
use std::cmp::Ordering;

#[derive(Debug, Deserialize)]
struct RouteEndpointCanonicalMetadata {
    kind: String,
    #[serde(default)]
    framework: Option<String>,
    method: String,
    path: String,
    #[serde(default)]
    raw_path: Option<String>,
    #[serde(default)]
    params: Vec<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    source_convention: Option<String>,
    #[serde(default)]
    provenance: Vec<String>,
}

pub(super) fn route_endpoint_metadata_from_canonical(
    raw: &str,
    node: &GraphNode,
    source_file: Option<&str>,
) -> serde_json::Result<RouteEndpointMetadataDto> {
    let canonical = serde_json::from_str::<RouteEndpointCanonicalMetadata>(raw)?;
    let kind = match canonical.kind.as_str() {
        "openapi_endpoint" => RouteEndpointKindDto::OpenapiEndpoint,
        _ => RouteEndpointKindDto::FrameworkRoute,
    };
    Ok(RouteEndpointMetadataDto {
        kind,
        framework: canonical.framework,
        method: canonical.method,
        path: canonical.path,
        raw_path: canonical.raw_path,
        params: canonical.params,
        source_file: source_file.map(ToOwned::to_owned),
        line: node.start_line,
        confidence: canonical.confidence,
        source_convention: canonical.source_convention,
        handler: None,
        provenance: canonical.provenance,
    })
}

fn route_endpoint_extraction_score_adjustment(canonical_id: Option<&str>) -> f32 {
    let Some(raw) = canonical_id.and_then(|value| value.strip_prefix("route_endpoint:")) else {
        return 0.0;
    };
    let Ok(canonical) = serde_json::from_str::<RouteEndpointCanonicalMetadata>(raw) else {
        return 0.0;
    };

    if canonical.provenance.iter().any(|entry| {
        matches!(
            entry.as_str(),
            "extraction:ast_indexed" | "extraction:tree_sitter_query"
        )
    }) {
        return 0.025;
    }
    if canonical.provenance.iter().any(|entry| {
        matches!(
            entry.as_str(),
            "extraction:text_only" | "extraction:lexical_fallback"
        )
    }) {
        return -0.025;
    }
    0.0
}

pub(super) fn route_endpoint_adjusted_search_score(score: f32, canonical_id: Option<&str>) -> f32 {
    (score + route_endpoint_extraction_score_adjustment(canonical_id)).max(0.0)
}

pub(super) fn route_endpoint_metadata_from_openapi_label(
    label: &str,
    node: &GraphNode,
    source_file: Option<&str>,
) -> Option<RouteEndpointMetadataDto> {
    let (method, path) = label.split_once(' ')?;
    Some(RouteEndpointMetadataDto {
        kind: RouteEndpointKindDto::OpenapiEndpoint,
        framework: None,
        method: method.to_ascii_uppercase(),
        path: path.to_string(),
        raw_path: Some(path.to_string()),
        params: route_endpoint_params(path),
        source_file: source_file.map(ToOwned::to_owned),
        line: node.start_line,
        confidence: Some("schema".to_string()),
        source_convention: Some("openapi".to_string()),
        handler: None,
        provenance: vec!["openapi".to_string()],
    })
}

fn route_endpoint_params(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| {
            let segment = segment.trim();
            segment
                .strip_prefix(':')
                .or_else(|| {
                    segment
                        .strip_prefix('{')
                        .and_then(|value| value.strip_suffix('}'))
                })
                .map(str::to_string)
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn route_handler_certainty_rank(
    certainty: Option<codestory_contracts::graph::ResolutionCertainty>,
) -> u8 {
    match certainty {
        Some(codestory_contracts::graph::ResolutionCertainty::Certain) => 3,
        Some(codestory_contracts::graph::ResolutionCertainty::Probable) => 2,
        Some(codestory_contracts::graph::ResolutionCertainty::Uncertain) => 1,
        None => 0,
    }
}

#[derive(Debug, Clone)]
pub(super) struct RouteHandlerCandidate {
    pub(super) edge: GraphEdge,
    pub(super) target: GraphNode,
}

pub(super) fn compare_optional_confidence_desc(left: Option<f32>, right: Option<f32>) -> Ordering {
    let left = left.filter(|value| value.is_finite());
    let right = right.filter(|value| value.is_finite());
    match (left, right) {
        (Some(left), Some(right)) => right.total_cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub(super) fn compare_route_handler_candidates(
    left: &RouteHandlerCandidate,
    right: &RouteHandlerCandidate,
) -> Ordering {
    // Persisted graph edges have unique IDs, so the edge ID tie-breaker makes
    // this a total order for every valid route-handler candidate set.
    compare_optional_confidence_desc(left.edge.confidence, right.edge.confidence)
        .then_with(|| {
            route_handler_certainty_rank(right.edge.certainty)
                .cmp(&route_handler_certainty_rank(left.edge.certainty))
        })
        .then(left.target.canonical_id.cmp(&right.target.canonical_id))
        .then(left.target.qualified_name.cmp(&right.target.qualified_name))
        .then(
            left.target
                .serialized_name
                .cmp(&right.target.serialized_name),
        )
        .then((left.target.kind as i32).cmp(&(right.target.kind as i32)))
        .then(left.target.file_node_id.cmp(&right.target.file_node_id))
        .then(left.target.start_line.cmp(&right.target.start_line))
        .then(left.target.start_col.cmp(&right.target.start_col))
        .then(left.target.end_line.cmp(&right.target.end_line))
        .then(left.target.end_col.cmp(&right.target.end_col))
        .then(left.target.id.cmp(&right.target.id))
        .then(left.edge.id.cmp(&right.edge.id))
        .then(left.edge.source.cmp(&right.edge.source))
        .then(left.edge.target.cmp(&right.edge.target))
        .then(left.edge.resolved_source.cmp(&right.edge.resolved_source))
        .then(left.edge.resolved_target.cmp(&right.edge.resolved_target))
        .then(left.edge.line.cmp(&right.edge.line))
        .then(
            left.edge
                .callsite_identity
                .cmp(&right.edge.callsite_identity),
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrameworkRouteCoverageEntry {
    framework: &'static str,
    language: &'static str,
    status: &'static str,
    coverage_evidence: &'static str,
    confidence_floor: &'static str,
    handler_link_support: &'static str,
    unsupported_patterns: &'static [&'static str],
    known_gaps: &'static [&'static str],
    promotable: bool,
}

const FRAMEWORK_ROUTE_COVERAGE_ENTRIES: &[FrameworkRouteCoverageEntry] = &[
    FrameworkRouteCoverageEntry {
        framework: "express",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_direct_handler_names_when_graph_resolution_succeeds",
        unsupported_patterns: &[
            "nested, injected, factory-returned, chained, and multi-target receivers are not promoted",
            "dynamic paths and middleware arrays do not produce handler claims",
            "inline and nested-call handlers do not produce name-based handler links",
        ],
        known_gaps: &[
            "mounted app and router prefixes are not globally propagated",
            "direct require('express')() construction is not promoted",
        ],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "react-router",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &[
            "loader/action route objects and nested config composition are partial",
        ],
        known_gaps: &["generated routes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "sveltekit",
        language: "svelte/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_server_method_exports",
        unsupported_patterns: &[
            "route groups and advanced matcher params are normalized conservatively",
        ],
        known_gaps: &["layout load propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nextjs",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_route_method_exports",
        unsupported_patterns: &["middleware rewrites and route groups require source review"],
        known_gaps: &["parallel routes and intercepting routes are not fully modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "remix",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_loader_action_exports",
        unsupported_patterns: &["route config composition and resource routes are partial"],
        known_gaps: &["flat-route edge cases need more real-repo probes"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "astro",
        language: "astro/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_endpoint_method_exports",
        unsupported_patterns: &["redirects and integration-generated routes are not expanded"],
        known_gaps: &["content collections do not create route nodes"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nuxt",
        language: "vue/javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "file_convention",
        handler_link_support: "probable_for_server_handlers",
        unsupported_patterns: &["route middleware and generated module routes are partial"],
        known_gaps: &["custom router options are not evaluated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fastify",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_direct_handler_names_when_graph_resolution_succeeds",
        unsupported_patterns: &[
            "nested, injected, factory-returned, chained, and multi-target receivers are not promoted",
            "dynamic paths, method arrays, and nested route builders are not promoted",
            "inline and wrapped handlers do not produce name-based handler links",
        ],
        known_gaps: &[
            "register() prefix propagation is not modeled",
            "schema and runtime middleware semantics are not evaluated",
        ],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "koa",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["router prefixes and middleware arrays are partial"],
        known_gaps: &["mounted router prefixes are not globally propagated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "hono",
        language: "javascript/typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["basePath/grouped routes are partial"],
        known_gaps: &["OpenAPI helper generated routes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "nestjs",
        language: "typescript",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "decorator",
        handler_link_support: "probable_for_controller_method",
        unsupported_patterns: &["global prefixes and dynamic decorator expressions are partial"],
        known_gaps: &["module graph prefix propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "django",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["include() trees and namespaced URLConfs are not fully expanded"],
        known_gaps: &["path converters beyond parameter names are not typed"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "flask",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "decorator",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["blueprint prefixes and dynamic method declarations are partial"],
        known_gaps: &["method lists are not fully enumerated"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fastapi",
        language: "python",
        status: "partial",
        coverage_evidence: "validated_by_tree_sitter_query_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_for_decorated_handler",
        unsupported_patterns: &[
            "path= keyword arguments and escaped non-raw string literals are not exact routes",
            "head/options/api_route/websocket decorators are not indexed",
            "chained or multi-target FastAPI/APIRouter construction is not promoted",
            "factory-returned or injected router receivers without module-scope construction are not claimed",
        ],
        known_gaps: &["include_router prefix propagation is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "rails",
        language: "ruby",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["resource expansion is not fully enumerated"],
        known_gaps: &["constraints/scopes are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "laravel",
        language: "php",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["controller arrays and route groups are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "spring",
        language: "java",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "annotation",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["class-level prefixes are not fully combined in every case"],
        known_gaps: &["composed annotations are not expanded"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "aspnet",
        language: "csharp",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "attribute",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["controller-level route templates are partial"],
        known_gaps: &["minimal API grouping is not fully modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "axum",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["nested routers and stateful route composition are partial"],
        known_gaps: &["Router::nest prefix propagation is limited"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "actix",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "probable_when_handler_name_resolves",
        unsupported_patterns: &["scoped services and macros are partial"],
        known_gaps: &["web::scope prefix propagation is limited"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "rocket",
        language: "rust",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "attribute",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["mount prefixes are not fully combined"],
        known_gaps: &["rank/format route attributes are not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "gin",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["router groups and middleware chains are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "chi",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["route groups and mounted subrouters are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "echo",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["group prefixes are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "fiber",
        language: "go",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed_text_only",
        unsupported_patterns: &["group prefixes and mounted apps are partial"],
        known_gaps: &["group middleware/prefix stacking is not modeled"],
        promotable: true,
    },
    FrameworkRouteCoverageEntry {
        framework: "vue-router",
        language: "vue",
        status: "partial",
        coverage_evidence: "validated_by_indexer_regression",
        confidence_floor: "heuristic",
        handler_link_support: "not_claimed",
        unsupported_patterns: &["imported route arrays and generated routes are partial"],
        known_gaps: &["Nuxt file routes are reported separately as nuxt"],
        promotable: true,
    },
];

pub(super) fn framework_route_coverage_matrix() -> Vec<FrameworkRouteCoverageDto> {
    FRAMEWORK_ROUTE_COVERAGE_ENTRIES
        .iter()
        .map(framework_route_coverage_dto)
        .collect()
}

fn framework_route_coverage_dto(entry: &FrameworkRouteCoverageEntry) -> FrameworkRouteCoverageDto {
    FrameworkRouteCoverageDto {
        framework: entry.framework.to_string(),
        language: entry.language.to_string(),
        status: entry.status.to_string(),
        coverage_evidence: entry.coverage_evidence.to_string(),
        confidence_floor: entry.confidence_floor.to_string(),
        handler_link_support: entry.handler_link_support.to_string(),
        unsupported_patterns: entry
            .unsupported_patterns
            .iter()
            .map(|value| value.to_string())
            .collect(),
        known_gaps: entry
            .known_gaps
            .iter()
            .map(|value| value.to_string())
            .collect(),
        promotable: entry.promotable,
    }
}

pub(super) struct LanguageSupportSummary {
    pub(super) support_mode: String,
    pub(super) evidence_tier: String,
    pub(super) claim_label: String,
}

pub(super) fn language_support_summary_for_language(language: &str) -> LanguageSupportSummary {
    language_support_profile_for_language_name(language)
        .map(|profile| LanguageSupportSummary {
            support_mode: profile.support_mode.as_str().to_string(),
            evidence_tier: profile.evidence_tier.as_str().to_string(),
            claim_label: profile.claim_label.to_string(),
        })
        .unwrap_or_else(|| LanguageSupportSummary {
            support_mode: "unknown".to_string(),
            evidence_tier: "unknown".to_string(),
            claim_label: "no support claim recorded".to_string(),
        })
}
