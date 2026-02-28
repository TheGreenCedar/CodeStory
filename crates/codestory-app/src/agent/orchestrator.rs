use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::trace::{TraceRecorder, field};
use crate::{
    AppController, FocusedSourceContext, LocalAgentResponse, agent_backend_label,
    build_local_agent_prompt, configured_agent_command, markdown_snippet, mermaid_flowchart,
    mermaid_gantt, mermaid_sequence,
};
use codestory_api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentResponseBlockDto,
    AgentResponseSectionDto, AgentRetrievalPolicyModeDto, AgentRetrievalStepKindDto, ApiError,
    GraphArtifactDto, GraphRequest, GraphResponse, NodeDetailsDto, NodeDetailsRequest,
    NodeOccurrencesRequest, ReadFileTextRequest, SearchHit, SearchRequest, TrailConfigDto,
    TrailFilterOptionsDto,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_RESULTS: u32 = 8;
const DEFAULT_MAX_EDGES: u32 = 260;
const LATENCY_PHASE_DEADLINE_MS: u128 = 7_000;
const DEFAULT_SLA_TARGET_MS: u32 = 18_000;
const SEARCH_LOOP_MAX_TERMS_LATENCY: usize = 3;
const SEARCH_LOOP_MAX_TERMS_COMPLETENESS: usize = 6;
const AGENT_TERM_PLANNER_MAX_TERMS: usize = 6;

#[derive(Debug, Clone, Default)]
struct RetrievalBundle {
    hits: Vec<SearchHit>,
    citations: Vec<AgentCitationDto>,
    graphs: Vec<GraphArtifactDto>,
    focus_node_id: Option<codestory_api::NodeId>,
    focused_node: Option<NodeDetailsDto>,
    primary_graph: Option<GraphResponse>,
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
    let mut trace = TraceRecorder::new(Some(DEFAULT_SLA_TARGET_MS));
    let ask_started_at = Instant::now();

    let mut bundle = execute_retrieval(controller, &req, &prompt, &resolved_profile, &mut trace)?;

    let source_context = maybe_read_source_context(
        controller,
        &prompt,
        &resolved_profile,
        ask_started_at,
        bundle.focused_node.as_ref(),
        &mut trace,
    );

    let mermaid_graphs = build_mermaid_artifacts(&resolved_profile, &prompt, &bundle, &mut trace);
    bundle.graphs.extend(mermaid_graphs);

    let local_agent_prompt = build_local_agent_prompt(
        &prompt,
        &bundle.hits,
        bundle.focused_node.as_ref(),
        source_context.as_ref(),
    );

    let local_agent_step = trace.start_step(
        AgentRetrievalStepKindDto::LocalAgent,
        vec![field("backend", format!("{:?}", req.connection.backend))],
    );
    let local_agent_result = controller.run_local_agent(&req.connection, &local_agent_prompt);
    match &local_agent_result {
        Ok(response) => trace.finish_ok(
            local_agent_step,
            vec![
                field("backend_label", response.backend_label),
                field("response_chars", response.markdown.len().to_string()),
            ],
        ),
        Err(error) => trace.finish_err(local_agent_step, error.message.clone()),
    }

    let synth_step = trace.start_step(
        AgentRetrievalStepKindDto::AnswerSynthesis,
        vec![field("citation_count", bundle.citations.len().to_string())],
    );

    let sections = build_sections(
        &prompt,
        &resolved_profile,
        &bundle,
        source_context.as_ref(),
        &req,
        &local_agent_result,
    );

    trace.finish_ok(
        synth_step,
        vec![
            field("section_count", sections.len().to_string()),
            field("graph_count", bundle.graphs.len().to_string()),
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

    let summary = summarize_response(&resolved_profile, &bundle, local_agent_result.as_ref());

    Ok(AgentAnswerDto {
        answer_id: request_id,
        prompt,
        summary,
        sections,
        citations: bundle.citations,
        graphs: bundle.graphs,
        retrieval_trace: trace_payload,
    })
}

fn execute_retrieval(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    resolved_profile: &ResolvedProfile,
    trace: &mut TraceRecorder,
) -> Result<RetrievalBundle, ApiError> {
    let mut bundle = RetrievalBundle::default();

    let search_step = trace.start_step(
        AgentRetrievalStepKindDto::Search,
        vec![field("query_chars", prompt.len().to_string())],
    );
    let mut hits = controller.search(SearchRequest {
        query: prompt.to_string(),
    })?;

    let max_results = req.max_results.unwrap_or(DEFAULT_MAX_RESULTS).clamp(1, 25) as usize;
    if hits.len() < max_results {
        let loop_budget = if matches!(
            resolved_profile.policy_mode,
            AgentRetrievalPolicyModeDto::CompletenessFirst
        ) {
            SEARCH_LOOP_MAX_TERMS_COMPLETENESS
        } else {
            SEARCH_LOOP_MAX_TERMS_LATENCY
        };

        let agent_terms = if should_use_agent_term_planner(prompt, hits.len()) {
            request_agent_search_terms(
                controller,
                req,
                prompt,
                &hits,
                trace,
                AGENT_TERM_PLANNER_MAX_TERMS,
            )
        } else {
            Vec::new()
        };
        let heuristic_terms = prompt_search_terms(prompt);
        let mut combined_terms = Vec::<String>::new();
        let mut seen_terms = HashSet::<String>::new();
        for term in agent_terms.into_iter().chain(heuristic_terms.into_iter()) {
            let key = term.to_ascii_lowercase();
            if seen_terms.insert(key) {
                combined_terms.push(term);
            }
        }

        let mut loop_runs = 0usize;
        for term in combined_terms.into_iter().take(loop_budget) {
            if hits.len() >= max_results {
                break;
            }

            match controller.search(SearchRequest {
                query: term.to_string(),
            }) {
                Ok(extra_hits) => {
                    loop_runs += 1;
                    merge_search_hits(&mut hits, extra_hits, max_results * 3);
                }
                Err(error) => {
                    trace.annotate(format!(
                        "Search loop term '{}' failed: {}",
                        term, error.message
                    ));
                }
            }
        }

        if loop_runs > 0 {
            trace.annotate(format!(
                "Search loop executed {} term query(ies).",
                loop_runs
            ));
        } else {
            trace.annotate("Search loop did not run additional term queries.");
        }
    }

    if hits.len() > max_results {
        hits.truncate(max_results);
    }

    trace.finish_ok(
        search_step,
        vec![
            field("hits", hits.len().to_string()),
            field("max_results", max_results.to_string()),
        ],
    );

    let focus_node_id = req
        .focus_node_id
        .clone()
        .or_else(|| hits.first().map(|hit| hit.node_id.clone()));

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
                node_filter: plan.node_filter.clone(),
                max_nodes: plan.max_nodes,
                layout_direction: codestory_api::LayoutDirection::Horizontal,
            };

            match controller.graph_trail(request) {
                Ok(trail) => {
                    trace.finish_ok(
                        trail_step,
                        vec![
                            field("nodes", trail.nodes.len().to_string()),
                            field("edges", trail.edges.len().to_string()),
                            field("truncated", trail.truncated.to_string()),
                        ],
                    );
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

    let edge_occurrences_step = trace.start_step(
        AgentRetrievalStepKindDto::EdgeOccurrences,
        vec![field(
            "enabled",
            resolved_profile.include_edge_occurrences.to_string(),
        )],
    );
    if !resolved_profile.include_edge_occurrences {
        trace.finish_skipped(
            edge_occurrences_step,
            "Edge occurrences are disabled for this profile.",
            Vec::new(),
        );
    } else if let Some(edge_id) = first_edge_id_from_graphs(&bundle.graphs) {
        match controller.edge_occurrences(codestory_api::EdgeOccurrencesRequest { id: edge_id }) {
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

    let citations = hits
        .iter()
        .map(|hit| AgentCitationDto {
            node_id: hit.node_id.clone(),
            display_name: hit.display_name.clone(),
            kind: hit.kind,
            file_path: hit.file_path.clone(),
            line: hit.line,
            score: hit.score,
        })
        .collect::<Vec<_>>();

    bundle.hits = hits;
    bundle.citations = citations;
    bundle.focus_node_id = focus_node_id;
    bundle.focused_node = focused_node;
    bundle.primary_graph = primary_graph;

    Ok(bundle)
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

fn maybe_read_source_context(
    controller: &AppController,
    prompt: &str,
    resolved_profile: &ResolvedProfile,
    ask_started_at: Instant,
    focused_node: Option<&NodeDetailsDto>,
    trace: &mut TraceRecorder,
) -> Option<FocusedSourceContext> {
    let source_step = trace.start_step(
        AgentRetrievalStepKindDto::SourceRead,
        vec![field(
            "enabled",
            resolved_profile.enable_source_reads.to_string(),
        )],
    );

    if !resolved_profile.enable_source_reads {
        trace.finish_skipped(
            source_step,
            "Source reads disabled by profile configuration.",
            Vec::new(),
        );
        return None;
    }

    if !needs_source_context(prompt) {
        trace.finish_skipped(
            source_step,
            "Prompt does not request source-level context.",
            Vec::new(),
        );
        return None;
    }

    if matches!(
        resolved_profile.policy_mode,
        AgentRetrievalPolicyModeDto::LatencyFirst
    ) && ask_started_at.elapsed().as_millis() > LATENCY_PHASE_DEADLINE_MS
    {
        trace.finish_truncated(
            source_step,
            "Skipped source read because latency-first phase budget was exceeded.",
            vec![field(
                "phase_deadline_ms",
                LATENCY_PHASE_DEADLINE_MS.to_string(),
            )],
        );
        trace.annotate("Latency-first cutoff skipped source reads.");
        return None;
    }

    let Some(node) = focused_node else {
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

    match controller.read_file_text(ReadFileTextRequest { path: path.clone() }) {
        Ok(file) => {
            let context = FocusedSourceContext {
                path,
                line,
                snippet: markdown_snippet(&file.text, Some(line), 6),
            };
            trace.finish_ok(
                source_step,
                vec![
                    field("path", context.path.clone()),
                    field("line", context.line.to_string()),
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

fn build_mermaid_artifacts(
    profile: &ResolvedProfile,
    prompt: &str,
    bundle: &RetrievalBundle,
    trace: &mut TraceRecorder,
) -> Vec<GraphArtifactDto> {
    let mermaid_step = trace.start_step(
        AgentRetrievalStepKindDto::MermaidSynthesis,
        vec![field("existing_graphs", bundle.graphs.len().to_string())],
    );

    let mut artifacts = Vec::new();

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
            codestory_api::AgentRetrievalPresetDto::Callflow
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

fn fallback_mermaid(prompt: &str, hit_count: usize) -> String {
    let prompt_summary = prompt
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "flowchart LR\n    A[\"Prompt\"] --> B[\"{}\"]\n    B --> C[\"Indexed hits: {}\"]\n    C --> D[\"Refine symbol names or run indexing\"]\n",
        sanitize_mermaid_text(&prompt_summary),
        hit_count
    )
}

fn sanitize_mermaid_text(input: &str) -> String {
    let mut sanitized = input.replace('"', "").replace('\n', " ").replace('\r', " ");
    sanitized = sanitized
        .chars()
        .map(|ch| if ch.is_ascii_control() { ' ' } else { ch })
        .collect::<String>();
    let collapsed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "request".to_string()
    } else {
        collapsed
    }
}

fn first_uml_graph(graphs: &[GraphArtifactDto]) -> Option<GraphResponse> {
    graphs.iter().find_map(|graph| match graph {
        GraphArtifactDto::Uml { graph, .. } => Some(graph.clone()),
        GraphArtifactDto::Mermaid { .. } => None,
    })
}

fn first_edge_id_from_graphs(graphs: &[GraphArtifactDto]) -> Option<codestory_api::EdgeId> {
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
    req: &AgentAskRequest,
    local_agent_result: &Result<LocalAgentResponse, ApiError>,
) -> Vec<AgentResponseSectionDto> {
    let mut sections = Vec::new();

    let mut analysis_blocks = Vec::new();
    match local_agent_result {
        Ok(agent) => {
            analysis_blocks.push(AgentResponseBlockDto::Markdown {
                markdown: format!("{}\n\n_Executed via `{}`._", agent.markdown, agent.command),
            });
        }
        Err(error) => {
            let backend_label = agent_backend_label(req.connection.backend);
            let command = configured_agent_command(&req.connection);
            analysis_blocks.push(AgentResponseBlockDto::Markdown {
                markdown: format!(
                    "Could not run local {} command `{}`.\\
\nReason: {}\\
\nContinuing with indexed DB-first retrieval evidence.",
                    backend_label, command, error.message
                ),
            });
        }
    }

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

    if bundle.hits.is_empty() {
        markdown.push_str(
            "\nNo indexed symbol matches found. Try symbol names, module paths, or re-run indexing.\n",
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
                "- **{}** [{:?}] score `{:.3}`{}",
                hit.display_name, hit.kind, hit.score, location
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

fn summarize_response(
    resolved_profile: &ResolvedProfile,
    bundle: &RetrievalBundle,
    local_agent_result: Result<&LocalAgentResponse, &ApiError>,
) -> String {
    match local_agent_result {
        Ok(agent) => format!(
            "{} analyzed {} indexed match(es) in {:?} mode and generated {} graph artifact(s).",
            agent.backend_label,
            bundle.hits.len(),
            resolved_profile.policy_mode,
            bundle.graphs.len()
        ),
        Err(_) => format!(
            "DB-first retrieval ({:?}/{:?}) returned {} indexed match(es) and {} graph artifact(s).",
            resolved_profile.preset,
            resolved_profile.policy_mode,
            bundle.hits.len(),
            bundle.graphs.len()
        ),
    }
}

fn next_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("ask-{}", nanos)
}

fn should_use_agent_term_planner(prompt: &str, hit_count: usize) -> bool {
    if hit_count >= 3 {
        return false;
    }
    prompt.split_whitespace().count() >= 4
}

fn build_term_planner_prompt(
    prompt: &str,
    existing_hits: &[SearchHit],
    max_terms: usize,
) -> String {
    let seed = existing_hits
        .iter()
        .take(5)
        .map(|hit| hit.display_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "You are selecting code-symbol search terms.\n\
User question: {prompt}\n\
Current symbol hits: {}\n\
Return ONLY a comma-separated list of up to {max_terms} concise code search terms (identifiers, modules, or API names). \
No explanation, no markdown, no numbering.",
        if seed.is_empty() {
            "(none)"
        } else {
            seed.as_str()
        }
    )
}

fn parse_agent_terms(markdown: &str, max_terms: usize) -> Vec<String> {
    let normalized = markdown
        .replace('\r', "\n")
        .replace('\n', ",")
        .replace(';', ",");

    let mut terms = Vec::new();
    let mut seen = HashSet::<String>::new();

    for raw in normalized.split(',') {
        let trimmed = raw
            .trim()
            .trim_matches(['`', '"', '\'', '“', '”', '‘', '’'])
            .trim_start_matches([
                '-', '*', '•', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', ')',
            ])
            .trim();

        if trimmed.len() < 3 || trimmed.len() > 80 {
            continue;
        }

        let has_signal = trimmed
            .chars()
            .any(|ch| ch.is_ascii_alphabetic() || ch == '_' || ch == ':');
        if !has_signal {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            terms.push(trimmed.to_string());
        }

        if terms.len() >= max_terms {
            break;
        }
    }

    terms
}

fn request_agent_search_terms(
    controller: &AppController,
    req: &AgentAskRequest,
    prompt: &str,
    existing_hits: &[SearchHit],
    trace: &mut TraceRecorder,
    max_terms: usize,
) -> Vec<String> {
    let planner_step = trace.start_step(
        AgentRetrievalStepKindDto::LocalAgent,
        vec![
            field("purpose", "term_planning"),
            field("existing_hits", existing_hits.len().to_string()),
            field("max_terms", max_terms.to_string()),
        ],
    );

    let planner_prompt = build_term_planner_prompt(prompt, existing_hits, max_terms);
    match controller.run_local_agent(&req.connection, &planner_prompt) {
        Ok(response) => {
            let terms = parse_agent_terms(&response.markdown, max_terms);
            trace.finish_ok(
                planner_step,
                vec![
                    field("backend_label", response.backend_label),
                    field("terms_count", terms.len().to_string()),
                ],
            );
            if terms.is_empty() {
                trace.annotate("Agent term planner returned no usable terms.");
            } else {
                trace.annotate(format!(
                    "Agent term planner proposed terms: {}",
                    terms.join(", ")
                ));
            }
            terms
        }
        Err(error) => {
            trace.finish_err(planner_step, error.message.clone());
            trace.annotate("Agent term planner failed; falling back to heuristic terms.");
            Vec::new()
        }
    }
}

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

fn merge_search_hits(into: &mut Vec<SearchHit>, additional: Vec<SearchHit>, max_candidates: usize) {
    let mut by_id = HashMap::<codestory_api::NodeId, SearchHit>::new();

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profiles::ResolvedProfile;

    fn latency_profile() -> ResolvedProfile {
        ResolvedProfile {
            preset: codestory_api::AgentRetrievalPresetDto::Architecture,
            policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
            trail_plans: Vec::new(),
            include_edge_occurrences: false,
            enable_source_reads: true,
        }
    }

    #[test]
    fn mermaid_builder_guarantees_fallback_diagram() {
        let mut trace = TraceRecorder::new(Some(DEFAULT_SLA_TARGET_MS));
        let bundle = RetrievalBundle::default();
        let artifacts =
            build_mermaid_artifacts(&latency_profile(), "inspect this", &bundle, &mut trace);

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
    fn should_use_agent_term_planner_for_sentence_queries_with_few_hits() {
        assert!(should_use_agent_term_planner(
            "How does the language parsing work in this repo?",
            0
        ));
        assert!(!should_use_agent_term_planner("parser", 0));
        assert!(!should_use_agent_term_planner(
            "How does the language parsing work in this repo?",
            4
        ));
    }

    #[test]
    fn parse_agent_terms_handles_bullets_and_commas() {
        let parsed = parse_agent_terms(
            "- parser_pipeline\n- tree_sitter::Parser, language_parsing, ast_builder",
            6,
        );
        assert_eq!(
            parsed,
            vec![
                "parser_pipeline".to_string(),
                "tree_sitter::Parser".to_string(),
                "language_parsing".to_string(),
                "ast_builder".to_string(),
            ]
        );
    }

    #[test]
    fn merge_search_hits_deduplicates_and_keeps_best_score() {
        let mut into = vec![SearchHit {
            node_id: codestory_api::NodeId("1".to_string()),
            display_name: "Parser".to_string(),
            kind: codestory_api::NodeKind::FUNCTION,
            file_path: None,
            line: None,
            score: 10.0,
        }];

        merge_search_hits(
            &mut into,
            vec![
                SearchHit {
                    node_id: codestory_api::NodeId("1".to_string()),
                    display_name: "Parser".to_string(),
                    kind: codestory_api::NodeKind::FUNCTION,
                    file_path: None,
                    line: None,
                    score: 42.0,
                },
                SearchHit {
                    node_id: codestory_api::NodeId("2".to_string()),
                    display_name: "LanguageParser".to_string(),
                    kind: codestory_api::NodeKind::MODULE,
                    file_path: None,
                    line: None,
                    score: 18.0,
                },
            ],
            10,
        );

        assert_eq!(into.len(), 2);
        assert_eq!(into[0].node_id.0, "1");
        assert_eq!(into[0].score, 42.0);
    }
}
