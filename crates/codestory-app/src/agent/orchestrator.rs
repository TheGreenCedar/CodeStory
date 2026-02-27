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
use std::fmt::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_RESULTS: u32 = 8;
const DEFAULT_MAX_EDGES: u32 = 260;
const LATENCY_PHASE_DEADLINE_MS: u128 = 7_000;
const DEFAULT_SLA_TARGET_MS: u32 = 18_000;

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
}
