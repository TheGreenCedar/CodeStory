use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::trace::{TraceRecorder, field};
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, fallback_mermaid,
    hybrid_retrieval_enabled, mermaid_flowchart, mermaid_gantt, mermaid_sequence,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentResponseBlockDto, AgentResponseModeDto,
    AgentResponseSectionDto, AgentRetrievalPolicyModeDto, AgentRetrievalStepKindDto, ApiError,
    EdgeId, GraphArtifactDto, GraphRequest, GraphResponse, GroundingBudgetDto, IndexFreshnessDto,
    IndexFreshnessStatusDto, NodeDetailsDto, NodeDetailsRequest, NodeId, NodeOccurrencesRequest,
    RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin, SearchRepoTextMode, SearchRequest,
    TrailConfigDto, TrailFilterOptionsDto,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_RESULTS: u32 = 8;
const DEFAULT_MAX_EDGES: u32 = 260;
const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const MIN_PHASE_DEADLINE_MS: u128 = 750;
const WEAK_INITIAL_HIT_COUNT: usize = 3;
const WEAK_INITIAL_TOP_SCORE: f32 = 0.30;
const WEAK_INITIAL_MIN_LEXICAL_ANCHOR: f32 = 0.01;
const WEAK_INITIAL_MIN_GRAPH_ANCHOR: f32 = 0.25;
const SOURCE_SNIPPET_TRUNCATION_SUFFIX: &str =
    "\n// ... source snippet truncated by investigation byte cap\n```";
const GRAPH_ARTIFACT_BUNDLE_BYTE_CAP: usize = 512 * 1024;
const RETRIEVAL_VERSION_HYBRID: &str = "hybrid-v1";
const RETRIEVAL_VERSION_LEXICAL_ROLLBACK: &str = "lexical-rollback-v1";

fn retrieval_version() -> &'static str {
    if hybrid_retrieval_enabled() {
        RETRIEVAL_VERSION_HYBRID
    } else {
        RETRIEVAL_VERSION_LEXICAL_ROLLBACK
    }
}

fn stale_freshness_annotation(freshness: &IndexFreshnessDto) -> Option<String> {
    if freshness.status != IndexFreshnessStatusDto::Stale {
        return None;
    }
    let samples = freshness
        .samples
        .iter()
        .map(|sample| format!("{:?}:{}", sample.kind, sample.path))
        .collect::<Vec<_>>();
    Some(format!(
        "Index freshness stale: changed={} new={} removed={}{}.",
        freshness.changed_file_count,
        freshness.new_file_count,
        freshness.removed_file_count,
        if samples.is_empty() {
            String::new()
        } else {
            format!(" samples={}", samples.join(", "))
        }
    ))
}

fn latency_budget_ms(req: &AgentAskRequest) -> u128 {
    req.latency_budget_ms
        .unwrap_or(DEFAULT_SLA_TARGET_MS)
        .clamp(1_000, 120_000) as u128
}

fn phase_deadline_ms(req: &AgentAskRequest, numerator: u128, denominator: u128) -> u128 {
    let budget = latency_budget_ms(req);
    let scaled = budget
        .saturating_mul(numerator)
        .checked_div(denominator.max(1))
        .unwrap_or(budget);
    scaled.max(MIN_PHASE_DEADLINE_MS).min(budget)
}

fn should_truncate_phase(
    resolved_profile: &ResolvedProfile,
    ask_started_at: Instant,
    deadline_ms: u128,
) -> bool {
    matches!(
        resolved_profile.policy_mode,
        AgentRetrievalPolicyModeDto::LatencyFirst
    ) && ask_started_at.elapsed().as_millis() > deadline_ms
}

#[derive(Debug, Clone, Default)]
struct RetrievalBundle {
    hits: Vec<SearchHit>,
    citations: Vec<AgentCitationDto>,
    graphs: Vec<GraphArtifactDto>,
    focus_node_id: Option<codestory_contracts::api::NodeId>,
    focused_node: Option<NodeDetailsDto>,
    primary_graph: Option<GraphResponse>,
    fallback_used: bool,
    repo_explanation_fallback_used: bool,
    repo_text_fallback_used: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct GraphArtifactCapStats {
    retained_bytes: usize,
    omitted_count: usize,
    truncated: bool,
}

pub(crate) fn agent_ask(
    controller: &AppController,
    req: AgentAskRequest,
) -> Result<AgentAnswerDto, ApiError> {
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(ApiError::invalid_argument("Prompt cannot be empty."));
    }

    let request_id = next_request_id();
    let resolved_profile = resolve_profile(&prompt, &req.retrieval_profile);
    let sla_target_ms = req
        .latency_budget_ms
        .unwrap_or(DEFAULT_SLA_TARGET_MS)
        .clamp(1_000, 120_000);
    let mut trace = TraceRecorder::new(Some(sla_target_ms));
    let ask_started_at = Instant::now();

    let mut bundle = execute_retrieval(
        controller,
        &req,
        &prompt,
        ask_started_at,
        &resolved_profile,
        &mut trace,
    )?;
    let freshness = match controller.index_freshness() {
        Ok(freshness) => {
            if let Some(annotation) = stale_freshness_annotation(&freshness) {
                trace.annotate(annotation);
            }
            Some(freshness)
        }
        Err(error) => {
            trace.annotate(format!("Index freshness not checked: {}", error.message));
            None
        }
    };

    let source_context = maybe_read_source_context(
        controller,
        SourceContextRequest {
            req: &req,
            prompt: &prompt,
            resolved_profile: &resolved_profile,
            ask_started_at,
            focused_node: bundle.focused_node.as_ref(),
            fallback_focus: bundle.fallback_used,
        },
        &mut trace,
    );

    let mermaid_graphs = build_mermaid_artifacts(
        &resolved_profile,
        &req,
        &prompt,
        ask_started_at,
        &bundle,
        &mut trace,
    );
    bundle.graphs.extend(mermaid_graphs);
    let graph_cap_stats = cap_graph_artifacts(&mut bundle.graphs, GRAPH_ARTIFACT_BUNDLE_BYTE_CAP);
    if graph_cap_stats.truncated {
        trace.annotate(format!(
            "Graph artifact bundle truncated at {} bytes; narrow focus or reduce trail depth for complete graph exports.",
            GRAPH_ARTIFACT_BUNDLE_BYTE_CAP
        ));
    }

    let synth_step = trace.start_step(
        AgentRetrievalStepKindDto::AnswerSynthesis,
        vec![field("citation_count", bundle.citations.len().to_string())],
    );

    let sections = build_sections(&prompt, &resolved_profile, &bundle, source_context.as_ref());

    trace.finish_ok(
        synth_step,
        vec![
            field("section_count", sections.len().to_string()),
            field("graph_count", bundle.graphs.len().to_string()),
            field(
                "graph_artifact_bytes",
                graph_cap_stats.retained_bytes.to_string(),
            ),
            field(
                "graph_artifact_byte_cap",
                GRAPH_ARTIFACT_BUNDLE_BYTE_CAP.to_string(),
            ),
            field(
                "graph_artifacts_omitted",
                graph_cap_stats.omitted_count.to_string(),
            ),
            field(
                "graph_artifact_truncated",
                graph_cap_stats.truncated.to_string(),
            ),
        ],
    );

    let mut trace_payload = trace.finish(
        request_id.clone(),
        resolved_profile.preset,
        resolved_profile.policy_mode,
    );

    if trace_payload.policy_mode == AgentRetrievalPolicyModeDto::CompletenessFirst
        && trace_payload.sla_missed
        && let Some(target_ms) = trace_payload.sla_target_ms
    {
        trace_payload.annotations.push(format!(
            "Completeness-first run exceeded SLA target ({} ms > {} ms).",
            trace_payload.total_latency_ms, target_ms
        ));
    }

    tracing::info!(
        request_id = %trace_payload.request_id,
        profile = ?trace_payload.resolved_profile,
        policy_mode = ?trace_payload.policy_mode,
        total_latency_ms = trace_payload.total_latency_ms,
        step_count = trace_payload.steps.len(),
        hit_count = bundle.hits.len(),
        graph_count = bundle.graphs.len(),
        "agent ask completed"
    );

    let summary = summarize_response(&resolved_profile, &bundle);

    Ok(AgentAnswerDto {
        answer_id: request_id,
        prompt,
        summary,
        freshness,
        sections,
        citations: bundle.citations,
        subgraph_ids: bundle
            .graphs
            .iter()
            .map(|graph| match graph {
                GraphArtifactDto::Uml { id, .. } => id.clone(),
                GraphArtifactDto::Mermaid { id, .. } => id.clone(),
            })
            .collect(),
        retrieval_version: retrieval_version().to_string(),
        graphs: bundle.graphs,
        retrieval_trace: trace_payload,
    })
}

fn cap_graph_artifacts(
    graphs: &mut Vec<GraphArtifactDto>,
    byte_cap: usize,
) -> GraphArtifactCapStats {
    let mut retained = Vec::with_capacity(graphs.len());
    let mut retained_bytes = 0usize;
    let mut omitted_count = 0usize;

    for graph in graphs.drain(..) {
        let encoded_bytes = serde_json::to_vec(&graph)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if retained_bytes.saturating_add(encoded_bytes) <= byte_cap {
            retained_bytes = retained_bytes.saturating_add(encoded_bytes);
            retained.push(graph);
        } else {
            omitted_count = omitted_count.saturating_add(1);
        }
    }

    *graphs = retained;
    GraphArtifactCapStats {
        retained_bytes,
        omitted_count,
        truncated: omitted_count > 0,
    }
}

fn execute_retrieval(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<RetrievalBundle, ApiError> {
    let mut bundle = RetrievalBundle::default();
    let semantic_required = hybrid_retrieval_enabled();

    let search_step = trace.start_step(
        AgentRetrievalStepKindDto::Search,
        vec![field("query_chars", prompt.len().to_string())],
    );
    let semantic_query_step = trace.start_step(
        AgentRetrievalStepKindDto::SemanticQueryEmbedding,
        vec![field("required", semantic_required.to_string())],
    );
    let semantic_candidates_step = trace.start_step(
        AgentRetrievalStepKindDto::SemanticCandidateRetrieval,
        vec![field("required", semantic_required.to_string())],
    );
    let hybrid_rerank_step = trace.start_step(
        AgentRetrievalStepKindDto::HybridRerank,
        vec![field("required", semantic_required.to_string())],
    );

    let max_results = req
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, resolved_profile.max_search_results) as usize;
    let mut scored_hits = match controller.search_hybrid_scored(
        SearchRequest {
            query: prompt.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: max_results as u32,
            hybrid_weights: None,
            hybrid_limits: None,
        },
        req.focus_node_id.clone(),
        max_results,
        req.hybrid_weights.clone(),
    ) {
        Ok(value) => value,
        Err(error) => {
            trace.finish_err(search_step, error.message.clone());
            trace.finish_err(semantic_query_step, error.message.clone());
            trace.finish_err(semantic_candidates_step, error.message.clone());
            trace.finish_err(hybrid_rerank_step, error.message.clone());
            return Err(error);
        }
    };
    let hits = scored_hits
        .iter()
        .map(|scored| scored.hit.clone())
        .collect::<Vec<_>>();

    trace.finish_ok(
        search_step,
        vec![
            field("hits", hits.len().to_string()),
            field(
                "accepted_hits",
                if should_investigate(resolved_profile)
                    && weak_initial_hits(prompt, &hits)
                    && !has_literal_fallback_signal(prompt)
                {
                    "0".to_string()
                } else {
                    hits.len().to_string()
                },
            ),
            field("max_results", max_results.to_string()),
            field("repo_text", "off_initial"),
        ],
    );
    if semantic_required {
        trace.finish_ok(
            semantic_query_step,
            vec![
                field("model_required", "local"),
                field("query_embedded", "true"),
            ],
        );
        trace.finish_ok(
            semantic_candidates_step,
            vec![field("candidates", scored_hits.len().to_string())],
        );
        trace.finish_ok(
            hybrid_rerank_step,
            vec![field("ranked", hits.len().to_string())],
        );
    } else {
        trace.finish_skipped(
            semantic_query_step,
            "Hybrid retrieval disabled by CODESTORY_HYBRID_RETRIEVAL_ENABLED=false.",
            Vec::new(),
        );
        trace.finish_skipped(
            semantic_candidates_step,
            "Hybrid retrieval disabled by CODESTORY_HYBRID_RETRIEVAL_ENABLED=false.",
            Vec::new(),
        );
        trace.finish_ok(
            hybrid_rerank_step,
            vec![field("ranked", hits.len().to_string())],
        );
    }

    let initial_hit_count = hits.len();
    let mut hits = hits;
    let literal_fallback_signal = has_literal_fallback_signal(prompt);
    let promotable_focus_available =
        req.focus_node_id.is_some() || investigation_focus_anchor(prompt, &hits).is_some();
    let mut expansion_added_hits = false;
    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && !promotable_focus_available
    {
        let expanded = match investigate_query_expansion(
            controller,
            req,
            prompt,
            max_results,
            ask_started_at,
            resolved_profile,
            trace,
        ) {
            Ok(expanded) => expanded,
            Err(error) => {
                trace.annotate(format!(
                    "Investigation query expansion failed; continuing with initial hits: {}",
                    error.message
                ));
                Vec::new()
            }
        };
        if !expanded.is_empty() {
            merge_scored_hits(&mut scored_hits, expanded, max_results);
            hits = scored_hits
                .iter()
                .map(|scored| scored.hit.clone())
                .collect::<Vec<_>>();
            bundle.fallback_used = true;
            expansion_added_hits = true;
        }

        if initial_hit_count == 0 && expansion_added_hits && !literal_fallback_signal {
            hits.clear();
            scored_hits.clear();
            trace.annotate(
                "Investigation discarded expansion-only hits for an unanchored natural-language query.",
            );
        }

        if weak_initial_hits(prompt, &hits) && literal_fallback_signal {
            let text_hits = match investigate_repo_text_fallback(
                controller,
                req,
                prompt,
                max_results,
                ask_started_at,
                resolved_profile,
                trace,
            ) {
                Ok(hits) => hits,
                Err(error) => {
                    trace.annotate(format!(
                        "Investigation repo-text fallback failed; continuing without file fallback: {}",
                        error.message
                    ));
                    Vec::new()
                }
            };
            if !text_hits.is_empty() {
                merge_search_hits(&mut hits, text_hits, max_results);
                bundle.fallback_used = true;
                bundle.repo_text_fallback_used = hits
                    .iter()
                    .any(|hit| hit.origin == SearchHitOrigin::TextMatch);
            }
        } else if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            if !hits.is_empty() {
                hits.clear();
                scored_hits.clear();
                trace.annotate(
                    "Investigation discarded low-confidence unanchored hits for a natural-language query.",
                );
            }
            trace.annotate(
                "Repo-text fallback skipped because the weak query did not contain a literal file/source token.",
            );
        } else if weak_initial_hits(prompt, &hits) {
            trace.annotate(
                "Investigation deferred a broad repo explanation prompt to grounding snapshot fallback.",
            );
        }

        if weak_initial_hits(prompt, &hits) && !is_repo_explanation_prompt(prompt) {
            trace.annotate(
                "Investigation low confidence gap after query expansion and repo-text fallback.",
            );
        }
    } else if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && promotable_focus_available
    {
        trace.annotate(
            "Investigation kept an explicit or prompt-anchored focus instead of broad repo-text fallback.",
        );
    }

    if should_investigate(resolved_profile)
        && weak_initial_hits(prompt, &hits)
        && is_repo_explanation_prompt(prompt)
    {
        let overview_hits = repo_explanation_grounding_hits(
            controller,
            req,
            max_results,
            ask_started_at,
            resolved_profile,
            trace,
        )?;
        if !overview_hits.is_empty() {
            hits = overview_hits;
            scored_hits.clear();
            bundle.fallback_used = true;
            bundle.repo_explanation_fallback_used = true;
            trace.annotate(
                "Investigation used grounding snapshot fallback for a broad repo explanation prompt.",
            );
        }
    }

    let focus_node_id = req
        .focus_node_id
        .clone()
        .or_else(|| investigation_focus_anchor(prompt, &hits))
        .or_else(|| {
            hits.iter()
                .find(|hit| hit.resolvable)
                .map(|hit| hit.node_id.clone())
        });

    let filter_step = trace.start_step(
        AgentRetrievalStepKindDto::TrailFilterOptions,
        vec![field("has_focus", focus_node_id.is_some().to_string())],
    );
    let filter_options = match controller.graph_trail_filter_options() {
        Ok(options) => {
            trace.finish_ok(
                filter_step,
                vec![
                    field("edge_kinds", options.edge_kinds.len().to_string()),
                    field("node_kinds", options.node_kinds.len().to_string()),
                ],
            );
            options
        }
        Err(error) => {
            trace.finish_err(filter_step, error.message.clone());
            trace
                .annotate("Trail filter options unavailable; continuing with unsanitized filters.");
            TrailFilterOptionsDto {
                node_kinds: Vec::new(),
                edge_kinds: Vec::new(),
            }
        }
    };

    let mut primary_graph: Option<GraphResponse> = None;

    if let Some(center_id) = focus_node_id.clone() {
        let neighborhood_step = trace.start_step(
            AgentRetrievalStepKindDto::Neighborhood,
            vec![field("center_id", center_id.0.clone())],
        );
        match controller.graph_neighborhood(GraphRequest {
            center_id,
            max_edges: Some(DEFAULT_MAX_EDGES),
        }) {
            Ok(neighborhood) => {
                trace.finish_ok(
                    neighborhood_step,
                    vec![
                        field("nodes", neighborhood.nodes.len().to_string()),
                        field("edges", neighborhood.edges.len().to_string()),
                        field("truncated", neighborhood.truncated.to_string()),
                    ],
                );

                primary_graph = Some(neighborhood.clone());
                bundle.graphs.push(GraphArtifactDto::Uml {
                    id: "uml-neighborhood".to_string(),
                    title: "Primary Neighborhood".to_string(),
                    graph: neighborhood,
                });
            }
            Err(error) => {
                trace.finish_err(neighborhood_step, error.message.clone());
                trace.annotate("Neighborhood retrieval failed; continuing with trail retrieval.");
            }
        }
    } else {
        let neighborhood_step = trace.start_step(
            AgentRetrievalStepKindDto::Neighborhood,
            vec![field("has_focus", "false")],
        );
        trace.finish_skipped(neighborhood_step, "No focus node selected.", Vec::new());
    }

    let sanitized_plans = resolved_profile
        .trail_plans
        .iter()
        .map(|plan| sanitize_plan_filters(plan, &filter_options))
        .collect::<Vec<_>>();

    if focus_node_id.is_none() {
        let trail_step = trace.start_step(
            AgentRetrievalStepKindDto::Trail,
            vec![field("plans", sanitized_plans.len().to_string())],
        );
        trace.finish_skipped(trail_step, "No focus node selected.", Vec::new());
    } else {
        for (idx, plan) in sanitized_plans.iter().enumerate() {
            let trail_step = trace.start_step(
                AgentRetrievalStepKindDto::Trail,
                vec![
                    field("index", idx.to_string()),
                    field("mode", format!("{:?}", plan.mode)),
                    field("depth", plan.depth.to_string()),
                    field("direction", format!("{:?}", plan.direction)),
                    field("max_nodes", plan.max_nodes.to_string()),
                ],
            );

            let root_id = focus_node_id.clone().expect("checked focus node");
            let request = TrailConfigDto {
                root_id,
                mode: plan.mode,
                target_id: None,
                depth: plan.depth,
                direction: plan.direction,
                caller_scope: plan.caller_scope,
                edge_filter: plan.edge_filter.clone(),
                show_utility_calls: true,
                hide_speculative: false,
                story: false,
                node_filter: plan.node_filter.clone(),
                max_nodes: plan.max_nodes,
                layout_direction: codestory_contracts::api::LayoutDirection::Horizontal,
            };

            match controller.graph_trail(request) {
                Ok(trail) => {
                    let trail_output = vec![
                        field("nodes", trail.nodes.len().to_string()),
                        field("edges", trail.edges.len().to_string()),
                        field("max_nodes", plan.max_nodes.to_string()),
                        field("truncated", trail.truncated.to_string()),
                        field("omitted_edges", trail.omitted_edge_count.to_string()),
                    ];
                    if trail.truncated {
                        trace.finish_truncated(
                            trail_step,
                            format!(
                                "Trail output hit max_nodes={}; narrow focus or lower depth.",
                                plan.max_nodes
                            ),
                            trail_output,
                        );
                        trace.annotate(format!(
                            "Trail {} was truncated at max_nodes={}.",
                            idx + 1,
                            plan.max_nodes
                        ));
                    } else {
                        trace.finish_ok(trail_step, trail_output);
                    }
                    bundle.graphs.push(GraphArtifactDto::Uml {
                        id: format!("uml-trail-{}", idx + 1),
                        title: format!("Trail {}", idx + 1),
                        graph: trail,
                    });
                }
                Err(error) => {
                    trace.finish_err(trail_step, error.message.clone());
                    trace.annotate(format!("Trail {} failed and was skipped.", idx + 1));
                }
            }
        }
    }

    let details_step = trace.start_step(
        AgentRetrievalStepKindDto::NodeDetails,
        vec![field("has_focus", focus_node_id.is_some().to_string())],
    );
    let focused_node = match focus_node_id.clone() {
        Some(id) => match controller.node_details(NodeDetailsRequest { id }) {
            Ok(details) => {
                trace.finish_ok(
                    details_step,
                    vec![
                        field("display_name", details.display_name.clone()),
                        field("kind", format!("{:?}", details.kind)),
                    ],
                );
                Some(details)
            }
            Err(error) => {
                trace.finish_err(details_step, error.message.clone());
                None
            }
        },
        None => {
            trace.finish_skipped(details_step, "No focus node selected.", Vec::new());
            None
        }
    };

    let occurrences_step = trace.start_step(
        AgentRetrievalStepKindDto::NodeOccurrences,
        vec![field("candidates", hits.len().min(3).to_string())],
    );
    let node_occurrence_deadline = phase_deadline_ms(req, 65, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, node_occurrence_deadline) {
        trace.finish_truncated(
            occurrences_step,
            "Skipped node occurrence lookups because latency budget was exceeded.",
            vec![field(
                "phase_deadline_ms",
                node_occurrence_deadline.to_string(),
            )],
        );
        trace.annotate("Latency-first cutoff skipped node occurrence lookups.");
    } else {
        let mut occurrence_count = 0usize;
        for hit in hits.iter().take(3) {
            match controller.node_occurrences(NodeOccurrencesRequest {
                id: hit.node_id.clone(),
            }) {
                Ok(occurrences) => {
                    occurrence_count += occurrences.len();
                }
                Err(error) => {
                    trace.annotate(format!(
                        "Node occurrence lookup failed for {}: {}",
                        hit.display_name, error.message
                    ));
                }
            }
        }
        trace.finish_ok(
            occurrences_step,
            vec![field("occurrence_count", occurrence_count.to_string())],
        );
    }

    let edge_occurrences_step = trace.start_step(
        AgentRetrievalStepKindDto::EdgeOccurrences,
        vec![field(
            "enabled",
            resolved_profile.include_edge_occurrences.to_string(),
        )],
    );
    let edge_occurrence_deadline = phase_deadline_ms(req, 75, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, edge_occurrence_deadline) {
        trace.finish_truncated(
            edge_occurrences_step,
            "Skipped edge occurrence lookup because latency budget was exceeded.",
            vec![field(
                "phase_deadline_ms",
                edge_occurrence_deadline.to_string(),
            )],
        );
        trace.annotate("Latency-first cutoff skipped edge occurrence lookups.");
    } else if !resolved_profile.include_edge_occurrences {
        trace.finish_skipped(
            edge_occurrences_step,
            "Edge occurrences are disabled for this profile.",
            Vec::new(),
        );
    } else if let Some(edge_id) = first_edge_id_from_graphs(&bundle.graphs) {
        match controller
            .edge_occurrences(codestory_contracts::api::EdgeOccurrencesRequest { id: edge_id })
        {
            Ok(occurrences) => {
                trace.finish_ok(
                    edge_occurrences_step,
                    vec![field("occurrence_count", occurrences.len().to_string())],
                );
            }
            Err(error) => {
                trace.finish_err(edge_occurrences_step, error.message.clone());
            }
        }
    } else {
        trace.finish_skipped(
            edge_occurrences_step,
            "No edges available for lookup.",
            Vec::new(),
        );
    }

    let primary_subgraph_id = bundle.graphs.first().map(|graph| match graph {
        GraphArtifactDto::Uml { id, .. } => id.clone(),
        GraphArtifactDto::Mermaid { id, .. } => id.clone(),
    });
    let include_structured_evidence =
        req.include_evidence || matches!(req.response_mode, AgentResponseModeDto::Structured);
    let scored_by_node = scored_hits
        .iter()
        .map(|scored| (scored.hit.node_id.clone(), scored))
        .collect::<HashMap<_, _>>();
    let citations = hits
        .iter()
        .map(|hit| {
            if let Some(scored) = scored_by_node.get(&hit.node_id) {
                to_citation(
                    scored,
                    primary_subgraph_id.as_deref(),
                    primary_graph.as_ref(),
                    include_structured_evidence,
                )
            } else {
                to_citation_from_hit(
                    hit,
                    primary_subgraph_id.as_deref(),
                    primary_graph.as_ref(),
                    include_structured_evidence,
                )
            }
        })
        .collect::<Vec<_>>();

    bundle.hits = hits;
    bundle.citations = citations;
    bundle.focus_node_id = focus_node_id;
    bundle.focused_node = focused_node;
    bundle.primary_graph = primary_graph;

    Ok(bundle)
}

fn to_citation(
    scored: &HybridSearchScoredHit,
    subgraph_id: Option<&str>,
    primary_graph: Option<&GraphResponse>,
    include_evidence: bool,
) -> AgentCitationDto {
    AgentCitationDto {
        node_id: scored.hit.node_id.clone(),
        display_name: scored.hit.display_name.clone(),
        kind: scored.hit.kind,
        file_path: scored.hit.file_path.clone(),
        line: scored.hit.line,
        score: scored.total_score,
        origin: scored.hit.origin,
        resolvable: scored.hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence {
            evidence_edge_ids_for_node(primary_graph, &scored.hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence.then_some(RetrievalScoreBreakdownDto {
            lexical: scored.lexical_score,
            semantic: scored.semantic_score,
            graph: scored.graph_score,
            total: scored.total_score,
        }),
    }
}

fn weak_initial_hits(prompt: &str, hits: &[SearchHit]) -> bool {
    let Some(top_hit) = hits.first() else {
        return true;
    };
    let prompt_terms = normalized_anchor_terms(prompt);
    if top_hit.score >= WEAK_INITIAL_TOP_SCORE && hit_has_indexed_anchor(top_hit, &prompt_terms) {
        return false;
    }

    hits.len() < WEAK_INITIAL_HIT_COUNT
        || top_hit.score < WEAK_INITIAL_TOP_SCORE
        || !hits
            .iter()
            .take(WEAK_INITIAL_HIT_COUNT)
            .any(|hit| hit_has_indexed_anchor(hit, &prompt_terms))
}

fn hit_has_indexed_anchor(hit: &SearchHit, prompt_terms: &HashSet<String>) -> bool {
    if hit.origin == SearchHitOrigin::TextMatch {
        return true;
    }
    if prompt_mentions_display_name(prompt_terms, &hit.display_name) {
        return true;
    }

    hit.score_breakdown
        .as_ref()
        .map(|breakdown| {
            breakdown.lexical > WEAK_INITIAL_MIN_LEXICAL_ANCHOR
                || breakdown.graph > WEAK_INITIAL_MIN_GRAPH_ANCHOR
        })
        .unwrap_or(hit.resolvable)
}

fn prompt_mentions_display_name(prompt_terms: &HashSet<String>, display_name: &str) -> bool {
    let display_terms = normalized_anchor_terms(display_name);
    !display_terms.is_empty() && display_terms.iter().all(|term| prompt_terms.contains(term))
}

fn investigation_focus_anchor(prompt: &str, hits: &[SearchHit]) -> Option<NodeId> {
    let prompt_terms = normalized_anchor_terms(prompt);
    hits.iter()
        .find(|hit| {
            hit.resolvable && prompt_mentions_display_name(&prompt_terms, &hit.display_name)
        })
        .map(|hit| hit.node_id.clone())
}

fn normalized_anchor_terms(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter_map(|term| {
            let term = term.trim().to_ascii_lowercase();
            (term.len() >= 3).then_some(term)
        })
        .collect()
}

fn should_investigate(profile: &ResolvedProfile) -> bool {
    profile.preset == codestory_contracts::api::AgentRetrievalPresetDto::Investigate
}

fn has_literal_fallback_signal(prompt: &str) -> bool {
    prompt.contains('`')
        || prompt.contains('/')
        || prompt.contains('\\')
        || prompt.contains("::")
        || prompt.contains(".rs")
        || prompt
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .any(|token| {
                token.contains('_')
                    || (token.len() >= 4
                        && token
                            .chars()
                            .filter(|ch| ch.is_ascii_alphabetic())
                            .all(|ch| ch.is_ascii_uppercase()))
            })
}

fn is_repo_explanation_prompt(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    let subject = lower.contains("repo") || lower.contains("project") || lower.contains("codebase");
    let intent = lower.contains("fit together")
        || lower.contains("how does")
        || lower.contains("explain")
        || lower.contains("overview")
        || lower.contains("architecture");
    subject && intent
}

fn repo_explanation_grounding_hits(
    controller: &AppController,
    req: &AgentAskRequest,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<SearchHit>, ApiError> {
    let step = trace.start_step(
        AgentRetrievalStepKindDto::QueryExpansion,
        vec![
            field("strategy", "grounding_snapshot"),
            field("max_results", max_results.to_string()),
        ],
    );
    let deadline_ms = phase_deadline_ms(req, 55, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, deadline_ms) {
        trace.finish_truncated(
            step,
            "Skipped grounding snapshot fallback because latency budget was exceeded.",
            vec![field("phase_deadline_ms", deadline_ms.to_string())],
        );
        return Ok(Vec::new());
    }

    let snapshot = match controller.grounding_snapshot(GroundingBudgetDto::Strict) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            trace.finish_err(step, error.message.clone());
            return Err(error);
        }
    };

    let hits = crate::grounding::grounding_explanation_search_hits(&snapshot, max_results);

    trace.finish_ok(
        step,
        vec![
            field("grounding_symbols", hits.len().to_string()),
            field("coverage_files", snapshot.coverage.total_files.to_string()),
            field(
                "coverage_symbols",
                snapshot.coverage.total_symbols.to_string(),
            ),
        ],
    );

    Ok(hits)
}

#[cfg(test)]
fn search_hit_from_grounding_symbol(
    symbol: &codestory_contracts::api::GroundingSymbolDigestDto,
) -> SearchHit {
    let (file_path, line) = symbol
        .node_ref
        .as_deref()
        .and_then(split_node_ref_location)
        .unwrap_or((None, symbol.line));
    SearchHit {
        node_id: symbol.id.clone(),
        display_name: symbol
            .label
            .split(" @ ")
            .next()
            .unwrap_or(symbol.label.as_str())
            .to_string(),
        kind: symbol.kind,
        file_path,
        line: symbol.line.or(line),
        score: 0.55,
        origin: SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        score_breakdown: Some(RetrievalScoreBreakdownDto {
            lexical: 0.35,
            semantic: 0.0,
            graph: 0.20,
            total: 0.55,
        }),
    }
}

#[cfg(test)]
fn split_node_ref_location(value: &str) -> Option<(Option<String>, Option<u32>)> {
    let mut parts = value.rsplitn(3, ':');
    let _name = parts.next()?;
    let line = parts.next()?.parse::<u32>().ok();
    let path = parts.next().map(ToOwned::to_owned);
    Some((path, line))
}

fn investigate_query_expansion(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<HybridSearchScoredHit>, ApiError> {
    let terms = prompt_search_terms(prompt)
        .into_iter()
        .take(4)
        .collect::<Vec<_>>();
    let expansion_step = trace.start_step(
        AgentRetrievalStepKindDto::QueryExpansion,
        vec![
            field("term_count", terms.len().to_string()),
            field("max_results", max_results.to_string()),
        ],
    );

    if terms.is_empty() {
        trace.finish_skipped(
            expansion_step,
            "No deterministic expansion terms extracted.",
            Vec::new(),
        );
        return Ok(Vec::new());
    }

    let expansion_deadline = phase_deadline_ms(req, 45, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, expansion_deadline) {
        trace.finish_truncated(
            expansion_step,
            "Skipped query expansion because latency budget was exceeded.",
            vec![field("phase_deadline_ms", expansion_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped investigation query expansion.");
        return Ok(Vec::new());
    }

    let mut expanded = Vec::new();
    for term in &terms {
        let hits = match controller.search_hybrid_scored(
            SearchRequest {
                query: term.clone(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: max_results as u32,
                hybrid_weights: None,
                hybrid_limits: None,
            },
            req.focus_node_id.clone(),
            max_results,
            req.hybrid_weights.clone(),
        ) {
            Ok(hits) => hits,
            Err(error) => {
                trace.finish_err(expansion_step, error.message.clone());
                return Err(error);
            }
        };
        expanded.extend(hits);
    }

    let hit_count = expanded.len();
    trace.finish_ok(
        expansion_step,
        vec![
            field("terms", terms.join(",")),
            field("hits", hit_count.to_string()),
        ],
    );
    Ok(expanded)
}

fn investigate_repo_text_fallback(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    max_results: usize,
    ask_started_at: Instant,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<Vec<SearchHit>, ApiError> {
    let fallback_step = trace.start_step(
        AgentRetrievalStepKindDto::RepoTextFallback,
        vec![
            field("query_chars", prompt.len().to_string()),
            field("max_results", max_results.to_string()),
        ],
    );

    let fallback_deadline = phase_deadline_ms(req, 55, 100);
    if should_truncate_phase(resolved_profile, ask_started_at, fallback_deadline) {
        trace.finish_truncated(
            fallback_step,
            "Skipped repo-text fallback because latency budget was exceeded.",
            vec![field("phase_deadline_ms", fallback_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped investigation repo-text fallback.");
        return Ok(Vec::new());
    }

    let results = match controller.search_results(SearchRequest {
        query: prompt.to_string(),
        repo_text: SearchRepoTextMode::On,
        limit_per_source: max_results as u32,
        hybrid_weights: None,
        hybrid_limits: None,
    }) {
        Ok(results) => results,
        Err(error) => {
            trace.finish_err(fallback_step, error.message.clone());
            return Err(error);
        }
    };
    let stats = results.repo_text_stats.clone();
    let mut hits = results.repo_text_hits;
    hits.truncate(max_results);
    let mut output = vec![
        field("repo_text_hits", hits.len().to_string()),
        field("origin", SearchHitOrigin::TextMatch.as_str()),
    ];
    if let Some(stats) = stats.as_ref() {
        output.push(field("scanned_files", stats.scanned_file_count.to_string()));
        output.push(field("scanned_bytes", stats.scanned_byte_count.to_string()));
        output.push(field("file_cap", stats.file_cap.to_string()));
        output.push(field("byte_cap", stats.byte_cap.to_string()));
        output.push(field("time_cap_ms", stats.time_cap_ms.to_string()));
        output.push(field("scan_truncated", stats.truncated.to_string()));
        if let Some(reason) = stats.reason.as_deref() {
            output.push(field("scan_reason", reason.to_string()));
        }
        if let Some(action) = stats.action.as_deref() {
            output.push(field("scan_action", action.to_string()));
        }
    }
    let scan_truncated = stats.as_ref().is_some_and(|stats| stats.truncated);
    if scan_truncated {
        trace.finish_truncated(
            fallback_step,
            "Repo-text fallback stopped at a configured scan cap.",
            output,
        );
    } else {
        trace.finish_ok(fallback_step, output);
    }
    if !hits.is_empty() {
        trace.annotate(
            "Repo-text fallback returned file/line evidence only; unresolved text hits are not treated as symbols.",
        );
    }
    if let Some(stats) = stats.as_ref()
        && stats.truncated
        && let Some(action) = stats.action.as_deref()
    {
        trace.annotate(format!("Repo-text fallback truncated: {action}"));
    }
    Ok(hits)
}

fn to_citation_from_hit(
    hit: &SearchHit,
    subgraph_id: Option<&str>,
    primary_graph: Option<&GraphResponse>,
    include_evidence: bool,
) -> AgentCitationDto {
    AgentCitationDto {
        node_id: hit.node_id.clone(),
        display_name: hit.display_name.clone(),
        kind: hit.kind,
        file_path: hit.file_path.clone(),
        line: hit.line,
        score: hit.score,
        origin: hit.origin,
        resolvable: hit.resolvable,
        subgraph_id: subgraph_id.map(ToOwned::to_owned),
        evidence_edge_ids: if include_evidence && hit.resolvable {
            evidence_edge_ids_for_node(primary_graph, &hit.node_id)
        } else {
            Vec::new()
        },
        retrieval_score_breakdown: include_evidence
            .then(|| hit.score_breakdown.clone())
            .flatten(),
    }
}

fn evidence_edge_ids_for_node(
    primary_graph: Option<&GraphResponse>,
    node_id: &codestory_contracts::api::NodeId,
) -> Vec<EdgeId> {
    let Some(graph) = primary_graph else {
        return Vec::new();
    };

    let mut edge_ids = graph
        .edges
        .iter()
        .filter(|edge| edge.source == *node_id || edge.target == *node_id)
        .map(|edge| edge.id.clone())
        .collect::<Vec<_>>();
    edge_ids.sort_by(|left, right| left.0.cmp(&right.0));
    edge_ids.truncate(12);
    edge_ids
}

fn sanitize_plan_filters(plan: &TrailPlan, options: &TrailFilterOptionsDto) -> TrailPlan {
    let mut sanitized = plan.clone();

    if !options.edge_kinds.is_empty() && !plan.edge_filter.is_empty() {
        sanitized
            .edge_filter
            .retain(|kind| options.edge_kinds.contains(kind));
    }

    if !options.node_kinds.is_empty() && !plan.node_filter.is_empty() {
        sanitized
            .node_filter
            .retain(|kind| options.node_kinds.contains(kind));
    }

    sanitized
}

struct SourceContextRequest<'a> {
    req: &'a AgentAskRequest,
    prompt: &'a str,
    resolved_profile: &'a ResolvedProfile,
    ask_started_at: Instant,
    focused_node: Option<&'a NodeDetailsDto>,
    fallback_focus: bool,
}

fn maybe_read_source_context(
    controller: &AppController,
    request: SourceContextRequest<'_>,
    trace: &mut TraceRecorder,
) -> Option<FocusedSourceContext> {
    let source_step = trace.start_step(
        AgentRetrievalStepKindDto::SourceRead,
        vec![field(
            "enabled",
            request.resolved_profile.enable_source_reads.to_string(),
        )],
    );

    if !request.resolved_profile.enable_source_reads {
        trace.finish_skipped(
            source_step,
            "Source reads disabled by profile configuration.",
            Vec::new(),
        );
        return None;
    }

    if !needs_source_context(request.prompt) && !request.fallback_focus {
        trace.finish_skipped(
            source_step,
            "Prompt does not request source-level context.",
            Vec::new(),
        );
        return None;
    }

    let source_deadline = phase_deadline_ms(request.req, 50, 100);
    if should_truncate_phase(
        request.resolved_profile,
        request.ask_started_at,
        source_deadline,
    ) {
        trace.finish_truncated(
            source_step,
            "Skipped source read because latency-first phase budget was exceeded.",
            vec![field("phase_deadline_ms", source_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped source reads.");
        return None;
    }

    let Some(node) = request.focused_node else {
        trace.finish_skipped(source_step, "No focused node available.", Vec::new());
        return None;
    };

    let (Some(path), Some(line)) = (node.file_path.clone(), node.start_line) else {
        trace.finish_skipped(
            source_step,
            "Focused node has no file path and line metadata.",
            Vec::new(),
        );
        return None;
    };

    match controller.bounded_file_snippet(
        &path,
        line,
        6,
        request.resolved_profile.max_source_bytes,
        SOURCE_SNIPPET_TRUNCATION_SUFFIX,
    ) {
        Ok((resolved_path, bounded)) => {
            let context = FocusedSourceContext {
                path: resolved_path,
                line,
                snippet: bounded.markdown,
            };
            trace.finish_ok(
                source_step,
                vec![
                    field("path", context.path.clone()),
                    field("line", context.line.to_string()),
                    field(
                        "max_source_bytes",
                        request.resolved_profile.max_source_bytes.to_string(),
                    ),
                    field("snippet_bytes", context.snippet.len().to_string()),
                    field("truncated", bounded.truncated.to_string()),
                ],
            );
            Some(context)
        }
        Err(error) => {
            trace.finish_err(source_step, error.message.clone());
            None
        }
    }
}

fn needs_source_context(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    [
        "code",
        "snippet",
        "implementation",
        "source",
        "line",
        "read",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

#[cfg(test)]
struct BoundedMarkdownSnippet {
    markdown: String,
    truncated: bool,
}

#[cfg(test)]
fn bounded_markdown_snippet(
    text: &str,
    focus_line: Option<u32>,
    context_lines: usize,
    max_bytes: usize,
) -> BoundedMarkdownSnippet {
    let mut snippet = crate::markdown_snippet(text, focus_line, context_lines);
    if snippet.len() <= max_bytes {
        return BoundedMarkdownSnippet {
            markdown: snippet,
            truncated: false,
        };
    }

    if max_bytes <= SOURCE_SNIPPET_TRUNCATION_SUFFIX.len() {
        snippet = SOURCE_SNIPPET_TRUNCATION_SUFFIX.to_string();
        while snippet.len() > max_bytes {
            snippet.pop();
        }
        return BoundedMarkdownSnippet {
            markdown: snippet,
            truncated: true,
        };
    }

    let content_budget = max_bytes - SOURCE_SNIPPET_TRUNCATION_SUFFIX.len();
    while snippet.len() > content_budget {
        snippet.pop();
    }
    snippet.push_str(SOURCE_SNIPPET_TRUNCATION_SUFFIX);
    debug_assert!(snippet.len() <= max_bytes);
    BoundedMarkdownSnippet {
        markdown: snippet,
        truncated: true,
    }
}
fn build_mermaid_artifacts(
    profile: &ResolvedProfile,
    req: &AgentAskRequest,
    prompt: &str,
    ask_started_at: Instant,
    bundle: &RetrievalBundle,
    trace: &mut TraceRecorder,
) -> Vec<GraphArtifactDto> {
    let mermaid_step = trace.start_step(
        AgentRetrievalStepKindDto::MermaidSynthesis,
        vec![field("existing_graphs", bundle.graphs.len().to_string())],
    );

    let mut artifacts = Vec::new();
    let mermaid_deadline = phase_deadline_ms(req, 85, 100);
    if should_truncate_phase(profile, ask_started_at, mermaid_deadline) {
        trace.finish_truncated(
            mermaid_step,
            "Skipped mermaid synthesis because latency budget was exceeded.",
            vec![field("phase_deadline_ms", mermaid_deadline.to_string())],
        );
        trace.annotate("Latency-first cutoff skipped mermaid synthesis.");
        return artifacts;
    }

    let primary_graph = bundle
        .primary_graph
        .clone()
        .or_else(|| first_uml_graph(&bundle.graphs));

    if let Some(graph) = primary_graph {
        artifacts.push(GraphArtifactDto::Mermaid {
            id: "mermaid-overview".to_string(),
            title: "Graph Overview".to_string(),
            diagram: "flowchart".to_string(),
            mermaid_syntax: mermaid_flowchart(&graph),
        });

        if matches!(
            profile.preset,
            codestory_contracts::api::AgentRetrievalPresetDto::Callflow
        ) {
            artifacts.push(GraphArtifactDto::Mermaid {
                id: "mermaid-sequence".to_string(),
                title: "Sequence Narrative".to_string(),
                diagram: "sequenceDiagram".to_string(),
                mermaid_syntax: mermaid_sequence(&graph),
            });
        }

        if prompt.to_ascii_lowercase().contains("timeline") {
            artifacts.push(GraphArtifactDto::Mermaid {
                id: "mermaid-timeline".to_string(),
                title: "Timeline".to_string(),
                diagram: "gantt".to_string(),
                mermaid_syntax: mermaid_gantt(&bundle.hits),
            });
        }
    }

    if artifacts.is_empty() {
        artifacts.push(GraphArtifactDto::Mermaid {
            id: "mermaid-fallback".to_string(),
            title: "Retrieval Fallback".to_string(),
            diagram: "flowchart".to_string(),
            mermaid_syntax: fallback_mermaid(prompt, bundle.hits.len()),
        });
    }

    trace.finish_ok(
        mermaid_step,
        vec![field("mermaid_count", artifacts.len().to_string())],
    );
    artifacts
}

fn first_uml_graph(graphs: &[GraphArtifactDto]) -> Option<GraphResponse> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Uml { graph, .. } => Some(graph.clone()),
        GraphArtifactDto::Mermaid { .. } => None,
    })
}

fn first_edge_id_from_graphs(
    graphs: &[GraphArtifactDto],
) -> Option<codestory_contracts::api::EdgeId> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Uml { graph, .. } => graph.edges.first().map(|edge| edge.id.clone()),
        GraphArtifactDto::Mermaid { .. } => None,
    })
}

fn build_sections(
    prompt: &str,
    resolved_profile: &ResolvedProfile,
    bundle: &RetrievalBundle,
    source_context: Option<&FocusedSourceContext>,
) -> Vec<AgentResponseSectionDto> {
    let mut sections = Vec::new();

    let mut analysis_blocks = vec![AgentResponseBlockDto::Markdown {
        markdown: "Answer assembled from indexed DB-first retrieval evidence.".to_string(),
    }];

    if let Some(primary_mermaid_id) = first_mermaid_graph_id(&bundle.graphs) {
        analysis_blocks.push(AgentResponseBlockDto::Mermaid {
            graph_id: primary_mermaid_id,
        });
    }

    sections.push(AgentResponseSectionDto {
        id: "analysis".to_string(),
        title: "Analysis".to_string(),
        blocks: analysis_blocks,
    });

    sections.push(AgentResponseSectionDto {
        id: "retrieval-evidence".to_string(),
        title: "Retrieval Evidence".to_string(),
        blocks: vec![AgentResponseBlockDto::Markdown {
            markdown: retrieval_markdown(prompt, resolved_profile, bundle, source_context),
        }],
    });

    let mermaid_ids = bundle
        .graphs
        .iter()
        .filter_map(|graph| match graph {
            GraphArtifactDto::Mermaid { id, .. } => Some(id.clone()),
            GraphArtifactDto::Uml { .. } => None,
        })
        .collect::<Vec<_>>();

    if !mermaid_ids.is_empty() {
        let mut blocks = vec![AgentResponseBlockDto::Markdown {
            markdown: "Mermaid diagrams generated from indexed graph retrieval.".to_string(),
        }];
        for graph_id in mermaid_ids {
            blocks.push(AgentResponseBlockDto::Mermaid { graph_id });
        }

        sections.push(AgentResponseSectionDto {
            id: "diagrams".to_string(),
            title: "Diagrams".to_string(),
            blocks,
        });
    }

    sections
}

fn retrieval_markdown(
    prompt: &str,
    profile: &ResolvedProfile,
    bundle: &RetrievalBundle,
    source_context: Option<&FocusedSourceContext>,
) -> String {
    let mut markdown = String::new();

    let _ = writeln!(markdown, "Prompt: **{}**", prompt.trim().replace('\n', " "));
    let _ = writeln!(
        markdown,
        "Resolved profile: `{:?}` (`{:?}` mode)",
        profile.preset, profile.policy_mode
    );
    let _ = writeln!(
        markdown,
        "Indexed hits: `{}` | Graph artifacts: `{}`",
        bundle.hits.len(),
        bundle.graphs.len()
    );

    if let Some(node) = bundle.focused_node.as_ref() {
        let _ = writeln!(
            markdown,
            "Focused symbol: **{}** (`{:?}`)",
            node.display_name, node.kind
        );
    }

    if let Some(source) = source_context {
        let _ = writeln!(
            markdown,
            "\nSource snippet from `{}`:{}:\n",
            source.path, source.line
        );
        markdown.push_str(&source.snippet);
        markdown.push('\n');
    }

    markdown.push_str("\nWhat I checked:\n");
    markdown.push_str("- Initial indexed-symbol search with current hybrid ranking.\n");
    if bundle.fallback_used {
        markdown.push_str("- Deterministic query expansion because initial hits were weak.\n");
    }
    if bundle.repo_text_fallback_used {
        markdown.push_str("- Repo-text/file fallback for literal file-line evidence.\n");
    }
    if bundle.repo_explanation_fallback_used {
        markdown.push_str("- Grounding snapshot fallback for broad repo overview evidence.\n");
    }
    if !bundle.fallback_used && should_investigate(profile) {
        markdown.push_str("- No fallback was needed because initial hits cleared the investigation confidence gate.\n");
    }

    if bundle.hits.is_empty() {
        markdown.push_str(
            "\nNo indexed symbol matches found. Try: symbol names, module paths, or re-run indexing.\n",
        );
    } else {
        markdown.push_str("\nTop indexed matches:\n");
        for hit in bundle.hits.iter().take(6) {
            let location = match (&hit.file_path, hit.line) {
                (Some(path), Some(line)) => format!(" ({}:{})", path, line),
                (Some(path), None) => format!(" ({})", path),
                _ => String::new(),
            };
            let _ = writeln!(
                markdown,
                "- **{}** [{:?}] origin `{}` resolvable `{}` score `{:.3}`{}",
                hit.display_name,
                hit.kind,
                hit.origin.as_str(),
                hit.resolvable,
                hit.score,
                location
            );
        }
    }

    if should_investigate(profile) && weak_initial_hits(prompt, &bundle.hits) {
        markdown.push_str("\nGaps:\n");
        markdown.push_str(
            "- Confidence is low: investigation mode could not find enough strong indexed-symbol evidence within its bounded search.\n",
        );
        if bundle.hits.iter().any(SearchHit::is_text_match) {
            markdown.push_str(
                "- Repo-text hits cite file/line locations only and were not treated as resolvable symbols.\n",
            );
        }
    }

    markdown
}

fn first_mermaid_graph_id(graphs: &[GraphArtifactDto]) -> Option<String> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Mermaid { id, .. } => Some(id.clone()),
        GraphArtifactDto::Uml { .. } => None,
    })
}

fn summarize_response(resolved_profile: &ResolvedProfile, bundle: &RetrievalBundle) -> String {
    format!(
        "DB-first retrieval ({:?}/{:?}) returned {} indexed match(es) and {} graph artifact(s).",
        resolved_profile.preset,
        resolved_profile.policy_mode,
        bundle.hits.len(),
        bundle.graphs.len()
    )
}

fn next_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("ask-{}", nanos)
}

#[allow(dead_code)]
fn prompt_search_terms(prompt: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "are",
        "as",
        "at",
        "be",
        "by",
        "can",
        "does",
        "for",
        "from",
        "how",
        "in",
        "is",
        "it",
        "of",
        "on",
        "or",
        "repo",
        "repository",
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

#[allow(dead_code)]
fn merge_search_hits(into: &mut Vec<SearchHit>, additional: Vec<SearchHit>, max_candidates: usize) {
    let mut by_id = HashMap::<codestory_contracts::api::NodeId, SearchHit>::new();

    for hit in into.drain(..) {
        by_id.insert(hit.node_id.clone(), hit);
    }

    for hit in additional {
        by_id
            .entry(hit.node_id.clone())
            .and_modify(|existing| {
                if hit.score > existing.score {
                    *existing = hit.clone();
                }
            })
            .or_insert(hit);
    }

    let mut merged = by_id.into_values().collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
    });
    merged.truncate(max_candidates);
    *into = merged;
}

fn merge_scored_hits(
    into: &mut Vec<HybridSearchScoredHit>,
    additional: Vec<HybridSearchScoredHit>,
    max_candidates: usize,
) {
    let mut by_id = HashMap::<codestory_contracts::api::NodeId, HybridSearchScoredHit>::new();

    for hit in into.drain(..) {
        by_id.insert(hit.hit.node_id.clone(), hit);
    }

    for hit in additional {
        by_id
            .entry(hit.hit.node_id.clone())
            .and_modify(|existing| {
                if hit.total_score > existing.total_score {
                    *existing = hit.clone();
                }
            })
            .or_insert(hit);
    }

    let mut merged = by_id.into_values().collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        right
            .total_score
            .partial_cmp(&left.total_score)
            .unwrap_or(Ordering::Equal)
    });
    merged.truncate(max_candidates);
    *into = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profiles::ResolvedProfile;

    fn latency_profile() -> ResolvedProfile {
        ResolvedProfile {
            preset: codestory_contracts::api::AgentRetrievalPresetDto::Architecture,
            policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
            trail_plans: Vec::new(),
            include_edge_occurrences: false,
            enable_source_reads: true,
            max_search_results: 25,
            max_source_bytes: 32 * 1024,
        }
    }

    fn test_search_hit(node_id: &str, score: f32) -> SearchHit {
        SearchHit {
            node_id: codestory_contracts::api::NodeId(node_id.to_string()),
            display_name: node_id.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
        }
    }

    fn test_semantic_only_hit(node_id: &str, score: f32) -> SearchHit {
        let mut hit = test_search_hit(node_id, score);
        hit.score_breakdown = Some(RetrievalScoreBreakdownDto {
            lexical: 0.0,
            semantic: score,
            graph: 0.0,
            total: score,
        });
        hit
    }

    #[test]
    fn investigation_mode_is_explicit_preset_only() {
        let mut profile = latency_profile();
        profile.policy_mode = AgentRetrievalPolicyModeDto::CompletenessFirst;
        assert!(!should_investigate(&profile));

        profile.preset = codestory_contracts::api::AgentRetrievalPresetDto::Investigate;
        assert!(should_investigate(&profile));
    }

    #[test]
    fn weak_initial_hits_use_normalized_search_scores() {
        assert!(!weak_initial_hits(
            "strong",
            &[
                test_search_hit("strong", 0.31),
                test_search_hit("second", 0.20),
                test_search_hit("third", 0.10),
            ]
        ));
        assert!(weak_initial_hits(
            "weak",
            &[
                test_search_hit("weak", 0.29),
                test_search_hit("second", 0.20),
                test_search_hit("third", 0.10),
            ]
        ));
        assert!(weak_initial_hits(
            "too_few",
            &[test_search_hit("too_few", 0.29)]
        ));
    }

    #[test]
    fn weak_initial_hits_treat_semantic_only_matches_as_low_confidence() {
        assert!(weak_initial_hits(
            "unrelated billing conveyor",
            &[
                test_semantic_only_hit("semantic_one", 0.90),
                test_semantic_only_hit("semantic_two", 0.80),
                test_semantic_only_hit("semantic_three", 0.70),
            ]
        ));
    }

    #[test]
    fn weak_initial_hits_accept_prompt_anchored_symbol_names() {
        assert!(!weak_initial_hits(
            "Where is exact_symbol_anchor used?",
            &[test_semantic_only_hit("exact_symbol_anchor", 0.90)]
        ));
    }

    #[test]
    fn investigation_focus_anchor_prefers_prompt_named_symbol() {
        let hit = test_semantic_only_hit("exact_symbol_anchor", 0.05);
        assert_eq!(
            investigation_focus_anchor("Explain exact_symbol_anchor", &[hit])
                .expect("prompt-named hit should become focus")
                .0,
            "exact_symbol_anchor"
        );
        assert!(
            investigation_focus_anchor(
                "Explain unrelated behavior",
                &[test_semantic_only_hit("exact_symbol_anchor", 0.90)]
            )
            .is_none()
        );
    }

    #[test]
    fn repo_explanation_prompt_detection_is_broad_but_not_symbolic() {
        assert!(is_repo_explanation_prompt(
            "How does this repo fit together?"
        ));
        assert!(is_repo_explanation_prompt(
            "Explain the project architecture"
        ));
        assert!(!is_repo_explanation_prompt(
            "Where is build_llm_symbol_doc_text used?"
        ));
    }

    #[test]
    fn grounding_symbol_fallback_hit_is_anchor_ranked() {
        let hit =
            search_hit_from_grounding_symbol(&codestory_contracts::api::GroundingSymbolDigestDto {
                id: NodeId("abc".to_string()),
                node_ref: Some("src/main.rs:42:AppController".to_string()),
                label: "AppController @ src/main.rs".to_string(),
                kind: codestory_contracts::api::NodeKind::STRUCT,
                line: None,
                member_count: None,
                summary: None,
                edge_digest: Vec::new(),
            });

        assert_eq!(hit.display_name, "AppController");
        assert_eq!(hit.file_path.as_deref(), Some("src/main.rs"));
        assert_eq!(hit.line, Some(42));
        assert!(!weak_initial_hits(
            "How does this repo fit together?",
            &[hit]
        ));
    }

    #[test]
    fn bounded_markdown_snippet_keeps_suffix_inside_byte_cap() {
        let source = (0..200)
            .map(|line| format!("let value_{line} = \"large source context\";\n"))
            .collect::<String>();

        let snippet = bounded_markdown_snippet(&source, Some(90), 90, 96);

        assert!(snippet.truncated);
        assert!(snippet.markdown.len() <= 96);
        assert!(snippet.markdown.contains("truncated"));
    }

    #[test]
    fn mermaid_builder_guarantees_fallback_diagram() {
        let mut trace = TraceRecorder::new(Some(DEFAULT_SLA_TARGET_MS));
        let bundle = RetrievalBundle::default();
        let artifacts = build_mermaid_artifacts(
            &latency_profile(),
            &AgentAskRequest {
                prompt: "inspect this".to_string(),
                retrieval_profile:
                    codestory_contracts::api::AgentRetrievalProfileSelectionDto::Auto,
                focus_node_id: None,
                max_results: None,
                response_mode: AgentResponseModeDto::Markdown,
                latency_budget_ms: None,
                include_evidence: true,
                hybrid_weights: None,
            },
            "inspect this",
            Instant::now(),
            &bundle,
            &mut trace,
        );

        assert_eq!(artifacts.len(), 1);
        assert!(matches!(artifacts[0], GraphArtifactDto::Mermaid { .. }));
    }

    #[test]
    fn source_context_keyword_gate_detects_code_requests() {
        assert!(needs_source_context(
            "show me the implementation and snippet"
        ));
        assert!(!needs_source_context(
            "summarize architecture at a high level"
        ));
    }

    #[test]
    fn prompt_search_terms_extracts_core_keywords() {
        let terms = prompt_search_terms("How does the language parsing work in this repo?");
        assert_eq!(terms, vec!["language".to_string(), "parsing".to_string()]);
    }

    #[test]
    fn merge_search_hits_deduplicates_and_keeps_best_score() {
        let mut into = vec![SearchHit {
            node_id: codestory_contracts::api::NodeId("1".to_string()),
            display_name: "Parser".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score: 10.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            score_breakdown: None,
        }];

        merge_search_hits(
            &mut into,
            vec![
                SearchHit {
                    node_id: codestory_contracts::api::NodeId("1".to_string()),
                    display_name: "Parser".to_string(),
                    kind: codestory_contracts::api::NodeKind::FUNCTION,
                    file_path: None,
                    line: None,
                    score: 42.0,
                    origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                    match_quality: None,
                    resolvable: true,
                    score_breakdown: None,
                },
                SearchHit {
                    node_id: codestory_contracts::api::NodeId("2".to_string()),
                    display_name: "LanguageParser".to_string(),
                    kind: codestory_contracts::api::NodeKind::MODULE,
                    file_path: None,
                    line: None,
                    score: 18.0,
                    origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
                    match_quality: None,
                    resolvable: true,
                    score_breakdown: None,
                },
            ],
            10,
        );

        assert_eq!(into.len(), 2);
        assert_eq!(into[0].node_id.0, "1");
        assert_eq!(into[0].score, 42.0);
    }

    #[test]
    fn evidence_edge_ids_are_sorted_and_filtered() {
        let graph = GraphResponse {
            center_id: codestory_contracts::api::NodeId("1".to_string()),
            nodes: Vec::new(),
            edges: vec![
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("8".to_string()),
                    source: codestory_contracts::api::NodeId("2".to_string()),
                    target: codestory_contracts::api::NodeId("3".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("3".to_string()),
                    source: codestory_contracts::api::NodeId("4".to_string()),
                    target: codestory_contracts::api::NodeId("2".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
                codestory_contracts::api::GraphEdgeDto {
                    id: EdgeId("9".to_string()),
                    source: codestory_contracts::api::NodeId("7".to_string()),
                    target: codestory_contracts::api::NodeId("8".to_string()),
                    kind: codestory_contracts::api::EdgeKind::CALL,
                    confidence: None,
                    certainty: None,
                    callsite_identity: None,
                    candidate_targets: Vec::new(),
                },
            ],
            truncated: false,
            canonical_layout: None,
            omitted_edge_count: 0,
        };

        let evidence = evidence_edge_ids_for_node(
            Some(&graph),
            &codestory_contracts::api::NodeId("2".to_string()),
        );
        let ids = evidence.into_iter().map(|id| id.0).collect::<Vec<_>>();
        assert_eq!(ids, vec!["3".to_string(), "8".to_string()]);
    }
}
