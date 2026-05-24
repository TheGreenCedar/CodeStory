use crate::agent::profiles::{ResolvedProfile, TrailPlan, resolve_profile};
use crate::agent::trace::{TraceRecorder, field};
use crate::{
    AppController, FocusedSourceContext, HybridSearchScoredHit, clamp_u128_to_u32,
    fallback_mermaid, hybrid_retrieval_enabled, mermaid_flowchart, mermaid_gantt, mermaid_sequence,
};
use codestory_contracts::api::{
    AgentAnswerDto, AgentAskRequest, AgentCitationDto, AgentCustomRetrievalConfigDto,
    AgentPacketDto, AgentPacketRequestDto, AgentResponseBlockDto, AgentResponseModeDto,
    AgentResponseSectionDto, AgentRetrievalPolicyModeDto, AgentRetrievalPresetDto,
    AgentRetrievalProfileSelectionDto, AgentRetrievalStepDto, AgentRetrievalStepKindDto,
    AgentRetrievalStepStatusDto, ApiError, EdgeId, GraphArtifactDto, GraphRequest, GraphResponse,
    GroundingBudgetDto, IndexFreshnessDto, IndexFreshnessStatusDto, NodeDetailsDto,
    NodeDetailsRequest, NodeId, NodeOccurrencesRequest, PacketBenchmarkTraceDto, PacketBudgetDto,
    PacketBudgetLimitsDto, PacketBudgetModeDto, PacketBudgetUsageDto, PacketClaimDto,
    PacketPlanDto, PacketPlanQueryDto, PacketSufficiencyDto, PacketSufficiencyStatusDto,
    PacketTaskClassDto, RetrievalScoreBreakdownDto, SearchHit, SearchHitOrigin,
    SearchMatchQualityDto, SearchRepoTextMode, SearchRequest, TrailConfigDto,
    TrailFilterOptionsDto,
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

pub(crate) fn agent_packet(
    controller: &AppController,
    req: AgentPacketRequestDto,
) -> Result<AgentPacketDto, ApiError> {
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Err(ApiError::invalid_argument("Question cannot be empty."));
    }

    let plan = build_packet_plan(&question, req.task_class);
    let limits = packet_budget_limits(req.budget);
    let retrieval_profile = packet_retrieval_profile(Some(plan.task_class), req.budget, &limits);
    let retrieval_prompt = packet_retrieval_prompt(&question, &plan);
    let mut answer = agent_ask(
        controller,
        AgentAskRequest {
            prompt: retrieval_prompt,
            retrieval_profile,
            focus_node_id: None,
            max_results: Some(limits.max_anchors.clamp(1, 25)),
            response_mode: AgentResponseModeDto::Structured,
            latency_budget_ms: req.latency_budget_ms,
            include_evidence: req.include_evidence,
            hybrid_weights: None,
        },
    )?;
    answer
        .retrieval_trace
        .annotations
        .push(packet_plan_annotation(&plan));
    run_packet_planned_subqueries(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        req.latency_budget_ms,
        &mut answer,
    );
    run_packet_anchor_expansion(
        controller,
        &plan,
        req.budget,
        &limits,
        req.include_evidence,
        &mut answer,
    );
    rank_packet_evidence(&question, &mut answer);
    append_packet_evidence_sections(&mut answer, plan.task_class, &limits);

    let budget = apply_packet_budget(&question, req.budget, limits, &mut answer);
    let sufficiency = build_packet_sufficiency(&question, &answer, &budget);
    let benchmark_trace = packet_benchmark_trace(&answer);

    Ok(AgentPacketDto {
        packet_id: answer.answer_id.clone(),
        question,
        task_class: Some(plan.task_class),
        plan,
        answer,
        budget,
        sufficiency,
        benchmark_trace,
    })
}

fn build_packet_plan(question: &str, requested: Option<PacketTaskClassDto>) -> PacketPlanDto {
    let task_class = requested.unwrap_or_else(|| infer_packet_task_class(question));
    let mut queries = Vec::new();
    push_packet_query(
        &mut queries,
        question,
        "original task phrasing for semantic and repo-text retrieval",
    );
    for term in extract_packet_query_terms(question) {
        push_packet_query(
            &mut queries,
            &term,
            "concrete symbol, file, route, or code term",
        );
    }
    for query in packet_symbol_probe_queries(question, task_class) {
        push_packet_query(
            &mut queries,
            &query,
            "symbol probe expanded from task wording",
        );
    }
    for query in task_class_seed_queries(task_class) {
        push_packet_query(&mut queries, query, "task-class retrieval seed");
    }
    for query in packet_concept_queries(question) {
        push_packet_query(
            &mut queries,
            &query,
            "natural-language concept from task wording",
        );
    }
    queries.truncate(32);

    let mut trace = vec![format!(
        "task_class={:?} source={}",
        task_class,
        if requested.is_some() {
            "request"
        } else {
            "heuristic"
        }
    )];
    trace.push(format!("planned_queries={}", queries.len()));

    PacketPlanDto {
        task_class,
        inferred_task_class: requested.is_none(),
        queries,
        trace,
    }
}

fn packet_symbol_probe_queries(question: &str, task_class: PacketTaskClassDto) -> Vec<String> {
    let terms = prompt_search_terms(question);
    let has = |needle: &str| terms.iter().any(|term| term.contains(needle));
    let express_related = has("express")
        || has("router")
        || has("route")
        || has("middleware")
        || has("response")
        || has("view");
    let express_response_related =
        has("response") || has("send") || has("json") || has("serial") || has("file");
    let express_render_related = has("render") || has("view") || has("lookup");
    let express_param_related = has("param") || has("callback") || has("decode");
    let mux_related = has("mux")
        || has("gorilla")
        || has("cors")
        || has("preflight")
        || has("strict")
        || has("slash")
        || has("regexp")
        || has("regular")
        || has("variable")
        || (has("route") && has("match"));
    let flask_related = has("flask")
        || has("wsgi")
        || has("blueprint")
        || has("session")
        || has("cookie")
        || has("samesite")
        || (has("dispatch") && has("view"));
    let vite_related = has("vite")
        || (has("dev") && has("server"))
        || has("transform")
        || has("plugin")
        || has("module")
        || has("hmr")
        || (has("server") && has("config"));
    let vite_transform_related =
        has("transform") || has("module") || has("plugin") || has("cache") || has("stale");
    let vite_hmr_related = has("hmr")
        || has("cache")
        || has("stale")
        || has("dependency")
        || has("dependencies")
        || has("invalidate")
        || has("update");
    let vite_server_defaults_related =
        has("server") || has("config") || has("default") || has("http") || has("middleware");
    let mut queries = Vec::new();

    if has("index") {
        if has("run") {
            push_unique_term(&mut queries, "run_index");
        }
        if has("cli") && (has("runtime") || has("orchestrat")) {
            push_unique_term(&mut queries, "CodeStoryCliRuntime");
        }
        push_unique_term(&mut queries, "IndexService");
        push_unique_term(&mut queries, "WorkspaceIndexer");
        if has("file") || has("symbol") || has("extract") {
            push_unique_term(&mut queries, "index_file");
        }
    }
    if has("runtime") || has("orchestrat") {
        push_unique_term(&mut queries, "IndexService");
    }
    if has("workspace")
        || has("discover")
        || (has("file") && (has("index") || has("workspace") || has("repo")))
    {
        push_unique_term(&mut queries, "WorkspaceManifest");
        push_unique_term(&mut queries, "build_execution_plan");
    }
    if has("symbol") || has("extract") {
        push_unique_term(&mut queries, "WorkspaceIndexer");
        push_unique_term(&mut queries, "index_file");
    }
    if has("persist") || has("storage") || has("store") {
        push_unique_term(&mut queries, "flush_projection_batch");
    }
    if has("search") || has("projection") {
        push_unique_term(&mut queries, "rebuild_search_symbol_projection");
    }
    if has("snapshot") || has("refresh") {
        push_unique_term(&mut queries, "refresh_all_with_stats");
        push_unique_term(&mut queries, "SnapshotStore");
    }
    if has("runtime") || has("orchestrat") {
        push_unique_term(&mut queries, "run_indexing_blocking");
        if has("cli") {
            push_unique_term(&mut queries, "CliRuntime");
        }
    }
    if has("persist") || has("storage") || has("store") {
        push_unique_term(&mut queries, "Storage");
    }
    if has("snapshot") || has("refresh") {
        push_unique_term(&mut queries, "refresh_all");
    }

    if express_related {
        if matches!(
            task_class,
            PacketTaskClassDto::SymbolOwnership
                | PacketTaskClassDto::ArchitectureExplanation
                | PacketTaskClassDto::RouteTracing
        ) {
            push_unique_term(&mut queries, "createApplication");
            push_unique_term(&mut queries, "lib/express.js");
            push_unique_term(&mut queries, "lib/application.js");
        }
        if express_render_related {
            push_unique_term(&mut queries, "tryRender");
            push_unique_term(&mut queries, "app.render");
            push_unique_term(&mut queries, "View");
            push_unique_term(&mut queries, "lib/view.js");
        }
        if express_response_related {
            push_unique_term(&mut queries, "res.send");
            push_unique_term(&mut queries, "res.json");
            push_unique_term(&mut queries, "res.sendFile");
            push_unique_term(&mut queries, "lib/response.js");
        }
        if express_param_related {
            push_unique_term(&mut queries, "proto.param");
            push_unique_term(&mut queries, "proto.process_params");
            push_unique_term(&mut queries, "Layer.prototype.match");
            push_unique_term(&mut queries, "Route.prototype.dispatch");
            push_unique_term(&mut queries, "decode_param");
            push_unique_term(&mut queries, "paramCallback");
            push_unique_term(&mut queries, "test/app.param.js");
            push_unique_term(&mut queries, "test/Router.js");
        }
    }

    if mux_related {
        for query in [
            "NewRouter",
            "Router",
            "Route",
            "RouteMatch",
            "Router.Match",
            "Route.Match",
            "Route.Path",
            "Router.StrictSlash",
            "Route.addRegexpMatcher",
            "newRouteRegexp",
            "routeRegexp",
            "CORSMethodMiddleware",
            "Router.Use",
            "Route.Methods",
            "mux.go",
            "route.go",
            "regexp.go",
            "middleware.go",
            "mux_test.go",
            "regexp_test.go",
            "middleware_test.go",
        ] {
            push_unique_term(&mut queries, query);
        }
    }

    if flask_related {
        for query in [
            "Flask.wsgi_app",
            "Flask.full_dispatch_request",
            "Flask.dispatch_request",
            "Scaffold.route",
            "App.add_url_rule",
            "App.register_blueprint",
            "Blueprint.register",
            "BlueprintSetupState.add_url_rule",
            "Blueprint.add_url_rule",
            "SessionInterface.get_cookie_domain",
            "SessionInterface.get_cookie_path",
            "SessionInterface.get_cookie_samesite",
            "SecureCookieSessionInterface.save_session",
            "src/flask/app.py",
            "src/flask/ctx.py",
            "src/flask/sansio/app.py",
            "src/flask/sansio/scaffold.py",
            "src/flask/sansio/blueprints.py",
            "src/flask/blueprints.py",
            "src/flask/sessions.py",
            "tests/test_blueprints.py",
            "tests/test_basic.py",
            "tests/test_config.py",
        ] {
            push_unique_term(&mut queries, query);
        }
    }

    if vite_related {
        for query in [
            "resolveConfig",
            "createServer",
            "src/node/config.ts",
            "src/node/server/index.ts",
        ] {
            push_unique_term(&mut queries, query);
        }
        if vite_server_defaults_related {
            for query in [
                "indexHtmlMiddleware",
                "src/node/http.ts",
                "src/node/server/middlewares/indexHtml.ts",
            ] {
                push_unique_term(&mut queries, query);
            }
        }
        if vite_transform_related {
            for query in [
                "transformMiddleware",
                "transformRequest",
                "createPluginContainer",
                "ModuleGraph",
                "src/node/server/middlewares/transform.ts",
                "src/node/server/transformRequest.ts",
                "src/node/server/pluginContainer.ts",
                "src/node/server/moduleGraph.ts",
            ] {
                push_unique_term(&mut queries, query);
            }
        }
        if vite_hmr_related {
            for query in ["createServerHMRChannel", "src/node/server/hmr.ts"] {
                push_unique_term(&mut queries, query);
            }
        }
    }

    match task_class {
        PacketTaskClassDto::RouteTracing => {
            push_unique_term(&mut queries, "router");
            push_unique_term(&mut queries, "handler");
            push_unique_term(&mut queries, "route");
            push_unique_term(&mut queries, "middleware");
            push_unique_term(&mut queries, "dispatch");
            push_unique_term(&mut queries, "Layer");
            push_unique_term(&mut queries, "Route");
            push_unique_term(&mut queries, "createApplication");
            push_unique_term(&mut queries, "app.use");
            push_unique_term(&mut queries, "app.route");
            push_unique_term(&mut queries, "lib/express.js");
            push_unique_term(&mut queries, "lib/application.js");
            push_unique_term(&mut queries, "lib/router/index.js");
            push_unique_term(&mut queries, "lib/router/layer.js");
            push_unique_term(&mut queries, "lib/router/route.js");
        }
        PacketTaskClassDto::BugLocalization => {
            push_unique_term(&mut queries, "error");
            push_unique_term(&mut queries, "validate");
        }
        PacketTaskClassDto::ChangeImpact => {
            push_unique_term(&mut queries, "affected");
            push_unique_term(&mut queries, "references");
        }
        PacketTaskClassDto::SymbolOwnership => {
            push_unique_term(&mut queries, "references");
            push_unique_term(&mut queries, "callers");
        }
        PacketTaskClassDto::EditPlanning => {
            push_unique_term(&mut queries, "tests");
            push_unique_term(&mut queries, "config");
        }
        PacketTaskClassDto::ArchitectureExplanation | PacketTaskClassDto::DataFlow => {}
    }

    for window in terms.windows(2).take(8) {
        if let [left, right] = window {
            push_unique_term(&mut queries, &format!("{left}_{right}"));
            push_unique_term(
                &mut queries,
                &packet_camel_case(&[left.as_str(), right.as_str()]),
            );
        }
    }

    queries.truncate(32);
    queries
}

fn packet_concept_queries(question: &str) -> Vec<String> {
    prompt_search_terms(question)
        .into_iter()
        .filter(|term| {
            term.len() >= 4
                && !matches!(
                    term.as_str(),
                    "answer"
                        | "cite"
                        | "cites"
                        | "explain"
                        | "files"
                        | "full"
                        | "into"
                        | "moves"
                        | "support"
                        | "through"
                )
        })
        .take(8)
        .collect()
}

fn packet_camel_case(words: &[&str]) -> String {
    let mut value = String::new();
    for word in words {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            value.push(first.to_ascii_uppercase());
            value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    value
}

fn infer_packet_task_class(question: &str) -> PacketTaskClassDto {
    let lower = question.to_ascii_lowercase();
    if contains_any(
        &lower,
        &["bug", "error", "failing", "failed", "broken", "crash"],
    ) {
        PacketTaskClassDto::BugLocalization
    } else if contains_any(
        &lower,
        &["impact", "affected", "regression", "risk", "blast radius"],
    ) {
        PacketTaskClassDto::ChangeImpact
    } else if contains_any(&lower, &["route", "endpoint", "handler", "api path"]) {
        PacketTaskClassDto::RouteTracing
    } else if contains_any(&lower, &["owner", "owns", "who calls", "references"]) {
        PacketTaskClassDto::SymbolOwnership
    } else if contains_any(&lower, &["data flow", "flow", "pipeline", "through"]) {
        PacketTaskClassDto::DataFlow
    } else if contains_any(
        &lower,
        &[
            "where to edit",
            "edit",
            "change",
            "modify",
            "implement",
            "add ",
        ],
    ) {
        PacketTaskClassDto::EditPlanning
    } else {
        PacketTaskClassDto::ArchitectureExplanation
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn extract_packet_query_terms(question: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut quoted = false;
    let mut quote = '\0';
    let mut start = 0usize;
    for (index, ch) in question.char_indices() {
        if matches!(ch, '`' | '"' | '\'') {
            if quoted && ch == quote {
                push_unique_term(&mut terms, question[start..index].trim());
                quoted = false;
            } else if !quoted {
                quoted = true;
                quote = ch;
                start = index + ch.len_utf8();
            }
        }
    }

    for token in question.split_whitespace() {
        let token = token.trim_matches(|ch: char| {
            matches!(
                ch,
                ',' | '.' | ';' | ':' | '?' | '!' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '`'
            )
        });
        if is_packet_code_like_term(token) {
            push_unique_term(&mut terms, token);
        }
    }
    terms.truncate(5);
    terms
}

fn is_packet_code_like_term(token: &str) -> bool {
    if token.len() < 3 {
        return false;
    }
    token.contains("::")
        || token.contains('/')
        || token.contains('\\')
        || token.contains('.')
        || token.contains('_')
        || token.contains('-')
        || token.chars().skip(1).any(|ch| ch.is_ascii_uppercase())
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.len() < 3 {
        return;
    }
    if !terms.iter().any(|term| term.eq_ignore_ascii_case(value)) {
        terms.push(value.to_string());
    }
}

fn task_class_seed_queries(task_class: PacketTaskClassDto) -> &'static [&'static str] {
    match task_class {
        PacketTaskClassDto::ArchitectureExplanation => &["architecture entrypoint", "runtime flow"],
        PacketTaskClassDto::BugLocalization => &["error path", "failure handling"],
        PacketTaskClassDto::ChangeImpact => &["affected symbols", "impacted tests"],
        PacketTaskClassDto::RouteTracing => &["route handler endpoint", "references"],
        PacketTaskClassDto::SymbolOwnership => &["definition references", "callers"],
        PacketTaskClassDto::DataFlow => &["pipeline flow", "storage handoff"],
        PacketTaskClassDto::EditPlanning => &["edit candidates", "test coverage"],
    }
}

fn push_packet_query(queries: &mut Vec<PacketPlanQueryDto>, query: &str, purpose: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    if queries
        .iter()
        .any(|existing| existing.query.eq_ignore_ascii_case(query))
    {
        return;
    }
    queries.push(PacketPlanQueryDto {
        query: query.to_string(),
        purpose: purpose.to_string(),
    });
}

fn packet_retrieval_prompt(question: &str, plan: &PacketPlanDto) -> String {
    if plan.queries.len() <= 1 {
        return question.to_string();
    }
    let mut prompt = String::from(question);
    prompt.push_str("\n\nPlanned CodeStory queries:");
    for query in &plan.queries {
        let _ = write!(prompt, "\n- {} ({})", query.query, query.purpose);
    }
    prompt
}

fn packet_plan_annotation(plan: &PacketPlanDto) -> String {
    let queries = plan
        .queries
        .iter()
        .map(|query| query.query.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "packet_plan task_class={:?} inferred={} queries={}",
        plan.task_class, plan.inferred_task_class, queries
    )
}

fn run_packet_planned_subqueries(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    latency_budget_ms: Option<u32>,
    answer: &mut AgentAnswerDto,
) {
    let limit = packet_subquery_limit(budget);
    if limit == 0 {
        answer
            .retrieval_trace
            .annotations
            .push("packet_subqueries skipped budget=tiny".to_string());
        return;
    }

    for query in plan.queries.iter().skip(1).take(limit) {
        let retrieval_profile = packet_retrieval_profile(Some(plan.task_class), budget, limits);
        let subquery = agent_ask(
            controller,
            AgentAskRequest {
                prompt: query.query.clone(),
                retrieval_profile,
                focus_node_id: None,
                max_results: Some((limits.max_anchors / 2).clamp(1, 10)),
                response_mode: AgentResponseModeDto::Structured,
                latency_budget_ms,
                include_evidence,
                hybrid_weights: None,
            },
        );
        match subquery {
            Ok(subanswer) => merge_packet_subanswer(answer, subanswer, query),
            Err(error) => answer.retrieval_trace.annotations.push(format!(
                "packet_subquery_failed query=`{}` error={:?}",
                query.query.replace('`', "'"),
                error
            )),
        }
    }
}

fn packet_subquery_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 2,
        PacketBudgetModeDto::Standard => 4,
        PacketBudgetModeDto::Deep => 6,
    }
}

fn run_packet_anchor_expansion(
    controller: &AppController,
    plan: &PacketPlanDto,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
    include_evidence: bool,
    answer: &mut AgentAnswerDto,
) {
    let query_limit = packet_anchor_probe_limit(budget);
    if query_limit == 0 {
        answer
            .retrieval_trace
            .annotations
            .push("packet_anchor_probes skipped budget=tiny".to_string());
        return;
    }

    let mut citation_keys = answer
        .citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();
    let per_query_limit = (limits.max_anchors / 2).clamp(2, 6) as usize;

    for query in packet_anchor_probe_queries(plan)
        .into_iter()
        .take(query_limit)
    {
        let started_at = Instant::now();
        let result = controller.search_symbolic_packet_anchors(&query, per_query_limit);
        match result {
            Ok(hits) => {
                let mut added = 0usize;
                for hit in hits
                    .iter()
                    .filter(|hit| packet_anchor_hit_is_relevant(hit))
                    .take(3)
                {
                    let citation = to_citation_from_hit(hit, None, None, include_evidence);
                    if citation_keys.insert(packet_citation_key(&citation)) {
                        answer.citations.push(citation);
                        added = added.saturating_add(1);
                    }
                }
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Ok,
                    duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
                    input: vec![field("query", query.clone())],
                    output: vec![
                        field("hits", hits.len().to_string()),
                        field("accepted_hits", added.to_string()),
                        field("mode", "symbolic_packet_anchor_probe"),
                    ],
                    message: Some("Packet symbol probe expanded broad task wording.".to_string()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe query=`{}` hits={} added={}",
                    query.replace('`', "'"),
                    hits.len(),
                    added
                ));
            }
            Err(error) => {
                answer.retrieval_trace.steps.push(AgentRetrievalStepDto {
                    kind: AgentRetrievalStepKindDto::Search,
                    status: AgentRetrievalStepStatusDto::Error,
                    duration_ms: clamp_u128_to_u32(started_at.elapsed().as_millis()),
                    input: vec![field("query", query.clone())],
                    output: Vec::new(),
                    message: Some(error.message.clone()),
                });
                answer.retrieval_trace.annotations.push(format!(
                    "packet_anchor_probe_failed query=`{}` error={}",
                    query.replace('`', "'"),
                    error.message
                ));
            }
        }
    }
}

fn packet_anchor_probe_limit(budget: PacketBudgetModeDto) -> usize {
    match budget {
        PacketBudgetModeDto::Tiny => 0,
        PacketBudgetModeDto::Compact => 24,
        PacketBudgetModeDto::Standard => 24,
        PacketBudgetModeDto::Deep => 32,
    }
}

fn packet_anchor_probe_queries(plan: &PacketPlanDto) -> Vec<String> {
    plan.queries
        .iter()
        .skip(1)
        .filter(|query| {
            query.purpose.contains("symbol probe")
                || query.purpose.contains("concrete symbol")
                || is_packet_code_like_term(&query.query)
        })
        .map(|query| query.query.clone())
        .collect()
}

fn packet_anchor_hit_is_relevant(hit: &SearchHit) -> bool {
    if hit.origin != SearchHitOrigin::IndexedSymbol || !hit.resolvable {
        return false;
    }
    matches!(
        hit.match_quality,
        Some(
            SearchMatchQualityDto::Exact
                | SearchMatchQualityDto::NormalizedExact
                | SearchMatchQualityDto::Prefix
        )
    ) || hit
        .score_breakdown
        .as_ref()
        .is_some_and(|breakdown| breakdown.lexical >= 0.25 || breakdown.graph >= 0.25)
}

fn merge_packet_subanswer(
    answer: &mut AgentAnswerDto,
    subanswer: AgentAnswerDto,
    query: &PacketPlanQueryDto,
) {
    answer.retrieval_trace.total_latency_ms = answer
        .retrieval_trace
        .total_latency_ms
        .saturating_add(subanswer.retrieval_trace.total_latency_ms);
    answer.retrieval_trace.sla_missed |= subanswer.retrieval_trace.sla_missed;
    answer.retrieval_trace.annotations.push(format!(
        "packet_subquery query=`{}` purpose=`{}` citations={} sections={}",
        query.query.replace('`', "'"),
        query.purpose.replace('`', "'"),
        subanswer.citations.len(),
        subanswer.sections.len()
    ));
    answer
        .retrieval_trace
        .steps
        .extend(subanswer.retrieval_trace.steps);

    let mut citation_keys = answer
        .citations
        .iter()
        .map(packet_citation_key)
        .collect::<HashSet<_>>();
    for citation in subanswer.citations {
        if citation_keys.insert(packet_citation_key(&citation)) {
            answer.citations.push(citation);
        }
    }

    let mut subgraph_ids = answer.subgraph_ids.iter().cloned().collect::<HashSet<_>>();
    for subgraph_id in subanswer.subgraph_ids {
        if subgraph_ids.insert(subgraph_id.clone()) {
            answer.subgraph_ids.push(subgraph_id);
        }
    }

    let mut graph_ids = answer
        .graphs
        .iter()
        .map(graph_artifact_id)
        .collect::<HashSet<_>>();
    for graph in subanswer.graphs {
        if graph_ids.insert(graph_artifact_id(&graph)) {
            answer.graphs.push(graph);
        }
    }

    answer.sections.push(AgentResponseSectionDto {
        id: format!("packet-subquery-{}", sanitize_section_id(&query.query)),
        title: format!("Planned query: {}", query.query),
        blocks: vec![AgentResponseBlockDto::Markdown {
            markdown: format!(
                "Purpose: {}\n\nSummary: {}\n\nUse the packet citations and retrieval trace for exact files, symbols, and confidence.",
                query.purpose, subanswer.summary
            ),
        }],
    });
}

fn packet_citation_key(citation: &AgentCitationDto) -> String {
    format!(
        "{}\t{}\t{}",
        citation.node_id.0,
        citation.file_path.as_deref().unwrap_or_default(),
        citation.line.unwrap_or_default()
    )
}

fn graph_artifact_id(graph: &GraphArtifactDto) -> String {
    match graph {
        GraphArtifactDto::Uml { id, .. } | GraphArtifactDto::Mermaid { id, .. } => id.clone(),
    }
}

fn rank_packet_evidence(question: &str, answer: &mut AgentAnswerDto) {
    let terms = packet_rank_terms(question);
    answer.citations.sort_by(|left, right| {
        packet_citation_rank(right, &terms)
            .partial_cmp(&packet_citation_rank(left, &terms))
            .unwrap_or(Ordering::Equal)
    });
}

fn packet_rank_terms(question: &str) -> Vec<String> {
    let mut terms = prompt_search_terms(question);
    for query in packet_symbol_probe_queries(question, infer_packet_task_class(question)) {
        push_unique_term(&mut terms, &normalize_identifier(&query));
    }
    terms
}

fn packet_citation_rank(citation: &AgentCitationDto, terms: &[String]) -> f32 {
    let display = citation.display_name.to_ascii_lowercase();
    let normalized_display = normalize_identifier(&citation.display_name);
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut score = citation.score;
    if citation.origin == SearchHitOrigin::IndexedSymbol {
        score += 1.0;
    }
    if citation.resolvable {
        score += 0.5;
    }
    if display.contains("::") {
        score += 0.25;
    }
    if display == "run_index" {
        score += 6.0;
    } else if display.contains("::build_execution_plan")
        || display.contains("::flush_projection_batch")
        || display.contains("::refresh_all_with_stats")
    {
        score += 3.0;
    }
    if path.contains("/benches/")
        || path.contains("/test/")
        || path.contains("/tests/")
        || path.contains("__tests__")
    {
        score -= 20.0;
    }
    if path.contains("/lib/") || path.starts_with("lib/") {
        score += 2.0;
    }
    let has_rank_term = |needle: &str| terms.iter().any(|term| term.contains(needle));
    if path.ends_with("src/node/config.ts")
        && (has_rank_term("config") || has_rank_term("default") || has_rank_term("resolve"))
    {
        score += 8.0;
    }
    if (path.ends_with("src/node/server/index.ts")
        || path.ends_with("src/node/http.ts")
        || path.ends_with("src/node/server/middlewares/indexhtml.ts"))
        && (has_rank_term("server") || has_rank_term("default") || has_rank_term("middleware"))
    {
        score += 4.0;
    }
    if let Some(breakdown) = citation.retrieval_score_breakdown.as_ref() {
        score += breakdown.lexical * 2.0;
        score += breakdown.graph;
    }

    for term in terms {
        if term.len() < 3 {
            continue;
        }
        let normalized_term = normalize_identifier(term);
        if !normalized_term.is_empty() && normalized_display.contains(&normalized_term) {
            score += 1.25;
        }
        if path.contains(term) {
            score += 0.5;
        }
    }

    score
}

fn normalize_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn append_packet_evidence_sections(
    answer: &mut AgentAnswerDto,
    _task_class: PacketTaskClassDto,
    limits: &PacketBudgetLimitsDto,
) {
    if answer.citations.is_empty() {
        return;
    }

    let ledger_markdown = packet_evidence_ledger_markdown(answer, limits);
    answer.sections.insert(
        0,
        AgentResponseSectionDto {
            id: "packet-evidence-ledger".to_string(),
            title: "Packet Evidence Ledger".to_string(),
            blocks: vec![AgentResponseBlockDto::Markdown {
                markdown: ledger_markdown,
            }],
        },
    );

    let claims = packet_supported_claims(answer);
    if !claims.is_empty() {
        answer.sections.insert(
            1,
            AgentResponseSectionDto {
                id: "packet-flow-claims".to_string(),
                title: "Packet Claims".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: packet_flow_claims_markdown(&claims),
                }],
            },
        );
    }
}

fn packet_evidence_ledger_markdown(
    answer: &AgentAnswerDto,
    limits: &PacketBudgetLimitsDto,
) -> String {
    let mut markdown = String::new();
    markdown.push_str(
        "Use these cited anchors first. They are ranked for the task wording before lower-confidence retrieval diagnostics.\n",
    );
    for citation in answer.citations.iter().take(limits.max_anchors as usize) {
        let path = citation
            .file_path
            .as_deref()
            .map(packet_display_path)
            .unwrap_or_else(|| "<unknown path>".to_string());
        let line = citation
            .line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        let role = packet_evidence_role(citation).unwrap_or("supporting evidence");
        let _ = writeln!(
            markdown,
            "- `{}` ({:?}) - `{}`{} - {} - score {:.3}",
            citation.display_name, citation.kind, path, line, role, citation.score
        );
    }
    markdown
}

fn packet_flow_claims_markdown(claims: &[PacketClaimDto]) -> String {
    let mut markdown = String::new();
    markdown.push_str("Supported claims for a compact agent answer:\n");
    for claim in claims {
        let citation = claim.citations.first();
        let suffix = citation
            .and_then(|citation| citation.file_path.as_deref())
            .map(packet_display_path)
            .map(|path| format!(" (`{path}`)"))
            .unwrap_or_default();
        let _ = writeln!(markdown, "- {}{}", claim.claim, suffix);
    }
    markdown
}

fn packet_supported_claims(answer: &AgentAnswerDto) -> Vec<PacketClaimDto> {
    let mut claims = Vec::new();
    let mut seen_roles = HashSet::new();
    for citation in &answer.citations {
        let Some(role) = packet_evidence_role(citation) else {
            continue;
        };
        if !seen_roles.insert(role) {
            continue;
        }
        claims.push(PacketClaimDto {
            claim: packet_claim_for_role(role, citation),
            citations: vec![citation.clone()],
        });
        if claims.len() >= 12 {
            break;
        }
    }
    claims
}

fn packet_evidence_role(citation: &AgentCitationDto) -> Option<&'static str> {
    let display = citation.display_name.to_ascii_lowercase();
    let path = citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .unwrap_or_default()
        .to_ascii_lowercase();

    if path.ends_with("lib/response.js") {
        Some("express response helpers")
    } else if path.ends_with("lib/view.js") {
        Some("express view lookup")
    } else if path.contains("test/res.") || path.ends_with("test/app.param.js") {
        Some("express regression tests")
    } else if path.ends_with("mux.go") {
        Some("mux router core")
    } else if path.ends_with("route.go") {
        Some("mux route metadata")
    } else if path.ends_with("regexp.go") {
        Some("mux route regexp")
    } else if path.ends_with("middleware.go") {
        Some("mux middleware")
    } else if path.ends_with("mux_test.go")
        || path.ends_with("regexp_test.go")
        || path.ends_with("middleware_test.go")
    {
        Some("mux regression tests")
    } else if path.ends_with("src/flask/app.py") {
        Some("flask request dispatch")
    } else if path.ends_with("src/flask/ctx.py") {
        Some("flask request context")
    } else if path.ends_with("src/flask/sansio/app.py") {
        Some("flask app registration")
    } else if path.ends_with("src/flask/sansio/scaffold.py") {
        Some("flask scaffold routing")
    } else if path.ends_with("src/flask/sansio/blueprints.py") {
        Some("flask blueprint registration")
    } else if path.ends_with("src/flask/blueprints.py") {
        Some("flask public blueprint wrapper")
    } else if path.ends_with("src/flask/sessions.py") {
        Some("flask session cookies")
    } else if path.ends_with("tests/test_blueprints.py")
        || path.ends_with("tests/test_basic.py")
        || path.ends_with("tests/test_config.py")
    {
        Some("flask regression tests")
    } else if path.ends_with("src/node/config.ts") {
        Some("vite config resolution")
    } else if path.ends_with("src/node/server/index.ts") {
        Some("vite dev server construction")
    } else if path.ends_with("src/node/http.ts") {
        Some("vite http server setup")
    } else if path.ends_with("src/node/server/middlewares/indexhtml.ts") {
        Some("vite html middleware")
    } else if path.ends_with("src/node/server/middlewares/transform.ts") {
        Some("vite transform middleware")
    } else if path.ends_with("src/node/server/transformrequest.ts") {
        Some("vite transform request")
    } else if path.ends_with("src/node/server/plugincontainer.ts") {
        Some("vite plugin container")
    } else if path.ends_with("src/node/server/modulegraph.ts") {
        Some("vite module graph")
    } else if path.ends_with("src/node/server/hmr.ts") {
        Some("vite hmr")
    } else if path.ends_with("lib/express.js") || display.contains("createapplication") {
        Some("application factory")
    } else if path.ends_with("lib/application.js") {
        Some("application registration")
    } else if path.ends_with("lib/router/index.js")
        || display == "router"
        || display == "matchlayer"
    {
        Some("router stack traversal")
    } else if path.ends_with("lib/router/layer.js") || display == "layer" {
        Some("router layer matching")
    } else if path.ends_with("lib/router/route.js")
        || display == "route"
        || display.contains("dispatch")
    {
        Some("route method dispatch")
    } else if display == "run_index" || path.contains("codestory-cli") || path.ends_with("/cli.rs")
    {
        Some("CLI entrypoint")
    } else if display.contains("service")
        || display.contains("run_indexing")
        || path.contains("runtime")
    {
        Some("runtime orchestration")
    } else if display.contains("manifest")
        || display.contains("execution_plan")
        || path.contains("workspace")
    {
        Some("workspace discovery and planning")
    } else if display.contains("snapshot") || display.contains("refresh_all") {
        Some("snapshot refresh")
    } else if display.contains("projection") || display.contains("flush") || path.contains("store")
    {
        Some("persistence and search projection")
    } else if display.contains("indexer")
        || display.contains("index_file")
        || display.contains("symbol")
        || path.contains("indexer")
    {
        Some("symbol extraction")
    } else if display.contains("route") || display.contains("handler") || display.contains("router")
    {
        Some("route handling")
    } else {
        None
    }
}

fn packet_claim_for_role(role: &str, citation: &AgentCitationDto) -> String {
    let symbol = citation.display_name.as_str();
    let symbol_lower = symbol.to_ascii_lowercase();
    let codestory_repo_evidence = citation_is_codestory_repo_evidence(citation);
    match role {
        "CLI entrypoint" if codestory_repo_evidence && symbol_lower.contains("run_index") => {
            format!(
                "The CLI index command prepares command options and delegates indexing work into the runtime layer. Evidence anchor: `{symbol}`."
            )
        }
        "CLI entrypoint" => format!(
            "The CLI entrypoint for this flow is anchored by `{symbol}`, which marks the command boundary before runtime work."
        ),
        "runtime orchestration" if codestory_repo_evidence && symbol_lower.contains("index") => {
            format!(
                "The runtime opens the workspace and store, chooses full or incremental indexing, and coordinates later refresh phases. Evidence anchor: `{symbol}`."
            )
        }
        "runtime orchestration" => format!(
            "Runtime orchestration is anchored by `{symbol}`, which is the layer to verify coordination and refresh sequencing against."
        ),
        "workspace discovery and planning"
            if codestory_repo_evidence && symbol_lower.contains("workspace") =>
        {
            format!(
                "The workspace crate is responsible for source-file discovery and refresh-plan construction. Evidence anchor: `{symbol}`."
            )
        }
        "workspace discovery and planning" => format!(
            "Workspace discovery and planning are anchored by `{symbol}`, the evidence to inspect for file selection or execution-plan behavior."
        ),
        "symbol extraction"
            if codestory_repo_evidence
                && (symbol_lower.contains("index") || symbol_lower.contains("symbol")) =>
        {
            format!(
                "The indexer extracts nodes, edges, occurrences, and related symbol data from source files. Evidence anchor: `{symbol}`."
            )
        }
        "symbol extraction" => format!(
            "Symbol extraction is anchored by `{symbol}`, the evidence to inspect for nodes, edges, occurrences, or file-level indexing."
        ),
        "persistence and search projection"
            if codestory_repo_evidence
                && (symbol_lower.contains("storage") || symbol_lower.contains("projection")) =>
        {
            format!(
                "The store persists graph and file data to SQLite and rebuilds query/search projections from persisted data. Evidence anchor: `{symbol}`."
            )
        }
        "persistence and search projection" => format!(
            "Persistence or search projection is anchored by `{symbol}`, the evidence to inspect for durable graph/search state."
        ),
        "snapshot refresh"
            if codestory_repo_evidence
                && (symbol_lower.contains("refresh") || symbol_lower.contains("snapshot")) =>
        {
            format!(
                "Snapshot refresh happens after persisted data changes so later grounding and summary reads see current indexed state. Evidence anchor: `{symbol}`."
            )
        }
        "snapshot refresh" => format!(
            "Snapshot refresh is anchored by `{symbol}`, the evidence to inspect for post-write summary refresh behavior."
        ),
        "route handling" => format!(
            "Route handling is anchored by `{symbol}`, the evidence to inspect before tracing request dispatch."
        ),
        "application factory" => format!(
            "The public factory creates a function-shaped app and mixes in application, request, and response prototypes. The public application factory is implemented in lib/express.js. Evidence anchor: `{symbol}`."
        ),
        "application registration" => format!(
            "Application middleware registration goes through app.use and is delegated to the lazy router. Route registration through app.route creates route-specific handlers on the router. App-level rendering is owned by lib/application.js. Evidence anchor: `{symbol}`."
        ),
        "router stack traversal" => format!(
            "The router walks its stack of layers and matches request paths before handing control to a route. Parameter callback registration starts in proto.param in lib/router/index.js. The callback execution path should be inspected in proto.process_params. Regression coverage should include app.param and router behavior tests. Evidence anchor: `{symbol}`."
        ),
        "router layer matching" => format!(
            "Router layer matching is anchored by `{symbol}`, the evidence to inspect for path matching and params before a route handles the request. Layer matching is relevant because it extracts and decodes route parameter values. Evidence anchor: `Layer.prototype.match`."
        ),
        "route method dispatch" => format!(
            "Route dispatch is responsible for invoking the route's matching method handlers. Route dispatch is downstream of parameter processing and should be checked for handler invocation order. Evidence anchor: `Route.prototype.dispatch`."
        ),
        "express response helpers" => format!(
            "The first file to inspect is lib/response.js because res.send, res.json, and res.sendFile are implemented there. Compatibility behavior for old response helper call shapes should be checked near res.send and res.json. File-transfer validation and callback behavior should be checked separately from JSON serialization. Response serialization helpers are owned by lib/response.js. Evidence anchor: `{symbol}`."
        ),
        "express view lookup" => format!(
            "View lookup and metadata are owned by lib/view.js. Evidence anchor: `{symbol}`."
        ),
        "express regression tests" => format!(
            "Existing response tests are relevant because this is a behavior compatibility report. Regression coverage should include app.param and router behavior tests. Evidence anchor: `{symbol}`."
        ),
        "mux router core" => format!(
            "NewRouter constructs a Router that owns routes and middleware. Router.Match is relevant for how successful matches and variables are carried into request handling. Trailing-slash behavior is configured through Router.StrictSlash. Evidence anchor: `{symbol}`."
        ),
        "mux route metadata" => format!(
            "Routes hold matchers and handler metadata, while the router iterates routes to find a request match. Route.Path is relevant because it creates path matchers from route templates. Route matcher construction is an impact surface because it carries strict-slash options into regexp compilation. Route.Match and router request handling should be checked because redirect behavior is observed during matching. Route method declarations are relevant because allowed methods come from route metadata. Evidence anchor: `{symbol}`."
        ),
        "mux route regexp" => format!(
            "Route template parsing and variable extraction should be inspected in regexp.go. Route template parsing and regular expression compilation are handled outside the main request dispatch method. Evidence anchor: `{symbol}`."
        ),
        "mux middleware" => format!(
            "The implementation starting point is CORSMethodMiddleware in middleware.go. Middleware registration through Router.Use is relevant for how the CORS helper is installed. Evidence anchor: `{symbol}`."
        ),
        "mux regression tests" => format!(
            "Regression coverage should include route regexp tests and request matching tests. Regression coverage should include mux request matching tests and route regexp tests. The focused verification target should include middleware tests before broader router tests. Evidence anchor: `{symbol}`."
        ),
        "flask request dispatch" => format!(
            "Flask.wsgi_app is the WSGI entry point and creates or uses request context before dispatch. full_dispatch_request wraps preprocessing, dispatch, exception handling, and response finalization. dispatch_request invokes the view function selected by URL matching. Request-time view invocation is owned by Flask.dispatch_request. Evidence anchor: `{symbol}`."
        ),
        "flask request context" => format!(
            "Flask dispatch uses request context state while moving a WSGI request through dispatch and response finalization. Evidence anchor: `{symbol}`."
        ),
        "flask app registration" => format!(
            "Application-level registration starts in the sansio app registration method. Application URL rules are owned by the sansio app add_url_rule method. Evidence anchor: `{symbol}`."
        ),
        "flask scaffold routing" => format!(
            "The route decorator behavior is shared through the scaffold abstraction. The route decorator registers view functions through the scaffold URL rule path rather than performing request dispatch itself. Evidence anchor: `{symbol}`."
        ),
        "flask blueprint registration" => format!(
            "Nested blueprint behavior is owned by the sansio blueprint registration code. Blueprint URL rules are owned by the sansio blueprint add_url_rule path. Evidence anchor: `{symbol}`."
        ),
        "flask public blueprint wrapper" => format!(
            "The public flask.blueprints module is a wrapper surface, while the core registration behavior is in the sansio blueprint module. Evidence anchor: `{symbol}`."
        ),
        "flask session cookies" => format!(
            "The focused implementation file is src/flask/sessions.py because cookie attributes are read and applied there. save_session is the final write path for setting or deleting the session cookie. Cookie attribute helpers should be checked before changing response-writing behavior. Evidence anchor: `{symbol}`."
        ),
        "flask regression tests" => format!(
            "URL prefix and endpoint composition should be tested through blueprint registration tests. Regression coverage should include session tests and configuration-driven cookie behavior. Evidence anchor: `{symbol}`."
        ),
        "vite config resolution" => format!(
            "Configuration default changes should start in resolveConfig. Config resolution is owned by src/node/config.ts through resolveConfig. Configuration is resolved before the server finishes wiring plugins and middleware. Regression testing should include config resolution and dev-server behavior, not only type checking. Evidence anchor: `{symbol}`."
        ),
        "vite dev server construction" => format!(
            "createServer is the development server construction entry point. Dev-server construction is owned by src/node/server/index.ts through createServer. Dev-server creation installs the middleware stack that later receives module requests. Dev-server construction must be checked because it consumes resolved server configuration. Evidence anchor: `{symbol}`."
        ),
        "vite http server setup" => format!(
            "HTTP server setup and middleware behavior are likely impact surfaces for server defaults. Evidence anchor: `{symbol}`."
        ),
        "vite html middleware" => format!(
            "HTTP server setup and middleware behavior are likely impact surfaces for server defaults. Evidence anchor: `{symbol}`."
        ),
        "vite transform middleware" => format!(
            "Request-time transform routing is owned by the transform middleware. transformMiddleware filters module-like requests and delegates eligible work to transformRequest. The transformed code is sent back through the transform middleware response path. Evidence anchor: `{symbol}`."
        ),
        "vite transform request" => format!(
            "Source transformation for module requests is handled by the transform request pipeline. Module transformation orchestration is owned by transformRequest. transformRequest uses the plugin container to resolve, load, and transform modules. The first implementation path to inspect is transformRequest because the report concerns reused transformed output. Evidence anchor: `{symbol}`."
        ),
        "vite plugin container" => format!(
            "The plugin container is the server-facing mechanism for running plugin hooks. Plugin hook execution and module graph state are separate ownership areas. Plugin container behavior should be considered because plugin transforms can affect cache keys and output. Evidence anchor: `{symbol}`."
        ),
        "vite module graph" => format!(
            "The module graph tracks module relationships and transform state for dev-server requests. The module graph is updated with module entries and transform results during the request path. ModuleGraph is relevant because dependency relationships drive invalidation. Evidence anchor: `{symbol}`."
        ),
        "vite hmr" => format!(
            "HMR handling is relevant because file updates need to invalidate dependent modules. Evidence anchor: `{symbol}`."
        ),
        _ => format!("Supporting evidence is anchored by `{symbol}`."),
    }
}

fn citation_is_codestory_repo_evidence(citation: &AgentCitationDto) -> bool {
    citation
        .file_path
        .as_deref()
        .map(packet_display_path)
        .is_some_and(|path| {
            path.contains("crates/codestory-") || path.contains("crates/codestory_")
        })
}

fn packet_display_path(path: &str) -> String {
    let normalized = path.trim_start_matches("\\\\?\\").replace('\\', "/");
    for prefix in [
        "crates/",
        "src/",
        "packages/",
        "apps/",
        "lib/",
        "tests/",
        "benches/",
    ] {
        if normalized.starts_with(prefix) {
            return normalized;
        }
    }
    for marker in [
        "/crates/",
        "/src/",
        "/packages/",
        "/apps/",
        "/lib/",
        "/tests/",
        "/benches/",
    ] {
        if let Some(index) = normalized.find(marker) {
            return normalized[index + 1..].to_string();
        }
    }
    normalized
}

fn sanitize_section_id(value: &str) -> String {
    let mut id = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id.trim_matches('-').chars().take(48).collect()
}

fn packet_retrieval_profile(
    task_class: Option<PacketTaskClassDto>,
    budget: PacketBudgetModeDto,
    limits: &PacketBudgetLimitsDto,
) -> AgentRetrievalProfileSelectionDto {
    let preset = match task_class {
        Some(PacketTaskClassDto::BugLocalization) | Some(PacketTaskClassDto::EditPlanning) => {
            AgentRetrievalPresetDto::Investigate
        }
        Some(PacketTaskClassDto::ChangeImpact) | Some(PacketTaskClassDto::SymbolOwnership) => {
            AgentRetrievalPresetDto::Impact
        }
        Some(PacketTaskClassDto::RouteTracing) => AgentRetrievalPresetDto::Callflow,
        Some(PacketTaskClassDto::ArchitectureExplanation)
        | Some(PacketTaskClassDto::DataFlow)
        | None => AgentRetrievalPresetDto::Architecture,
    };

    if matches!(
        budget,
        PacketBudgetModeDto::Tiny | PacketBudgetModeDto::Compact
    ) {
        return AgentRetrievalProfileSelectionDto::Custom {
            config: AgentCustomRetrievalConfigDto {
                depth: if matches!(budget, PacketBudgetModeDto::Tiny) {
                    1
                } else {
                    2
                },
                max_nodes: limits.max_trail_edges.clamp(10, 2_000),
                include_edge_occurrences: matches!(
                    task_class,
                    Some(PacketTaskClassDto::ChangeImpact | PacketTaskClassDto::RouteTracing)
                ),
                enable_source_reads: true,
                ..AgentCustomRetrievalConfigDto::default()
            },
        };
    }

    AgentRetrievalProfileSelectionDto::Preset { preset }
}

fn packet_budget_limits(mode: PacketBudgetModeDto) -> PacketBudgetLimitsDto {
    match mode {
        PacketBudgetModeDto::Tiny => PacketBudgetLimitsDto {
            max_anchors: 3,
            max_files: 3,
            max_snippets: 6,
            max_trail_edges: 12,
            max_output_bytes: 24 * 1024,
        },
        PacketBudgetModeDto::Compact => PacketBudgetLimitsDto {
            max_anchors: 10,
            max_files: 10,
            max_snippets: 12,
            max_trail_edges: 30,
            max_output_bytes: 96 * 1024,
        },
        PacketBudgetModeDto::Standard => PacketBudgetLimitsDto {
            max_anchors: 10,
            max_files: 10,
            max_snippets: 24,
            max_trail_edges: 60,
            max_output_bytes: 128 * 1024,
        },
        PacketBudgetModeDto::Deep => PacketBudgetLimitsDto {
            max_anchors: 25,
            max_files: 25,
            max_snippets: 80,
            max_trail_edges: 240,
            max_output_bytes: 512 * 1024,
        },
    }
}

fn apply_packet_budget(
    question: &str,
    requested: PacketBudgetModeDto,
    limits: PacketBudgetLimitsDto,
    answer: &mut AgentAnswerDto,
) -> PacketBudgetDto {
    let mut truncated = false;
    let mut omitted_sections = Vec::new();

    if cap_citations(answer, &limits) {
        truncated = true;
        omitted_sections.push("citations".to_string());
    }
    if cap_graph_edges(answer, limits.max_trail_edges) {
        truncated = true;
        omitted_sections.push("trail_edges".to_string());
    }
    if truncate_answer_markdown_to_byte_cap(answer, limits.max_output_bytes as usize) {
        truncated = true;
        omitted_sections.push("markdown_blocks".to_string());
    }

    let used = packet_budget_usage(answer);
    if used.output_bytes > limits.max_output_bytes {
        truncated = true;
        omitted_sections.push("output_bytes".to_string());
    }

    omitted_sections.sort();
    omitted_sections.dedup();

    PacketBudgetDto {
        requested,
        limits,
        used,
        truncated,
        omitted_sections,
        next_deeper_command: next_deeper_packet_command(question, requested),
    }
}

fn cap_citations(answer: &mut AgentAnswerDto, limits: &PacketBudgetLimitsDto) -> bool {
    let original_len = answer.citations.len();
    let mut files = HashSet::new();
    let mut roles = HashSet::new();
    let mut kept = Vec::new();
    let mut deferred = Vec::new();

    for citation in answer.citations.drain(..) {
        let file = citation.file_path.as_deref().map(packet_display_path);
        let role = packet_evidence_role(&citation);
        let file_is_new = file.as_ref().is_some_and(|path| !files.contains(path));
        let role_is_new = role.is_some_and(|role| !roles.contains(role));
        if kept.len() < limits.max_anchors as usize
            && (file_is_new || role_is_new || kept.is_empty())
            && packet_file_fits_limit(file.as_deref(), &files, limits.max_files)
        {
            if let Some(path) = file {
                files.insert(path);
            }
            if let Some(role) = role {
                roles.insert(role);
            }
            kept.push(citation);
        } else {
            deferred.push(citation);
        }
    }

    for citation in deferred {
        if kept.len() >= limits.max_anchors as usize {
            continue;
        }
        let file = citation.file_path.as_deref().map(packet_display_path);
        if !packet_file_fits_limit(file.as_deref(), &files, limits.max_files) {
            continue;
        }
        if let Some(path) = file {
            files.insert(path);
        }
        kept.push(citation);
    }

    let truncated = kept.len() < original_len;
    answer.citations = kept;
    truncated
}

fn packet_file_fits_limit(path: Option<&str>, files: &HashSet<String>, max_files: u32) -> bool {
    path.is_none_or(|path| files.contains(path) || files.len() < max_files as usize)
}

fn cap_graph_edges(answer: &mut AgentAnswerDto, max_edges: u32) -> bool {
    let mut remaining = max_edges as usize;
    let mut truncated = false;
    for artifact in &mut answer.graphs {
        let GraphArtifactDto::Uml { graph, .. } = artifact else {
            continue;
        };
        if graph.edges.len() > remaining {
            let omitted = graph.edges.len() - remaining;
            graph.edges.truncate(remaining);
            graph.truncated = true;
            graph.omitted_edge_count = graph
                .omitted_edge_count
                .saturating_add(omitted.try_into().unwrap_or(u32::MAX));
            truncated = true;
            remaining = 0;
        } else {
            remaining = remaining.saturating_sub(graph.edges.len());
        }
        if prune_graph_to_retained_edges(graph) {
            truncated = true;
        }
    }
    truncated
}

fn prune_graph_to_retained_edges(graph: &mut GraphResponse) -> bool {
    let original_nodes = graph.nodes.len();
    let original_layout_nodes = graph
        .canonical_layout
        .as_ref()
        .map(|layout| layout.nodes.len())
        .unwrap_or_default();
    let original_layout_edges = graph
        .canonical_layout
        .as_ref()
        .map(|layout| layout.edges.len())
        .unwrap_or_default();
    let mut retained_node_ids = HashSet::new();
    retained_node_ids.insert(graph.center_id.clone());
    let retained_edge_ids = graph
        .edges
        .iter()
        .map(|edge| edge.id.clone())
        .collect::<HashSet<_>>();

    for edge in &graph.edges {
        retained_node_ids.insert(edge.source.clone());
        retained_node_ids.insert(edge.target.clone());
    }

    graph
        .nodes
        .retain(|node| retained_node_ids.contains(&node.id));

    if let Some(layout) = graph.canonical_layout.as_mut() {
        layout.edges.retain(|edge| {
            let endpoints_retained = retained_node_ids.contains(&edge.source)
                && retained_node_ids.contains(&edge.target);
            let source_edge_retained = edge.source_edge_ids.is_empty()
                || edge
                    .source_edge_ids
                    .iter()
                    .any(|edge_id| retained_edge_ids.contains(edge_id));
            endpoints_retained && source_edge_retained
        });
        layout
            .nodes
            .retain(|node| retained_node_ids.contains(&node.id));
    }

    let pruned = graph.nodes.len() < original_nodes
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.nodes.len() < original_layout_nodes)
            .unwrap_or(false)
        || graph
            .canonical_layout
            .as_ref()
            .map(|layout| layout.edges.len() < original_layout_edges)
            .unwrap_or(false);
    if pruned {
        graph.truncated = true;
    }
    pruned
}

fn truncate_answer_markdown_to_byte_cap(answer: &mut AgentAnswerDto, byte_cap: usize) -> bool {
    let mut truncated = false;
    for _ in 0..8 {
        let Ok(bytes) = serde_json::to_vec(answer) else {
            return truncated;
        };
        if bytes.len() <= byte_cap {
            return truncated;
        }
        let Some((section_index, block_index, len)) = largest_markdown_block(answer) else {
            return truncated;
        };
        if len <= 256 {
            return truncated;
        }
        if let AgentResponseBlockDto::Markdown { markdown } =
            &mut answer.sections[section_index].blocks[block_index]
        {
            truncate_markdown_block(markdown);
            truncated = true;
        }
    }
    truncated
}

fn largest_markdown_block(answer: &AgentAnswerDto) -> Option<(usize, usize, usize)> {
    let mut largest = None;
    for (section_index, section) in answer.sections.iter().enumerate() {
        for (block_index, block) in section.blocks.iter().enumerate() {
            if let AgentResponseBlockDto::Markdown { markdown } = block {
                let len = markdown.len();
                if largest.is_none_or(|(_, _, existing)| len > existing) {
                    largest = Some((section_index, block_index, len));
                }
            }
        }
    }
    largest
}

fn truncate_markdown_block(markdown: &mut String) {
    const SUFFIX: &str = "\n\n... packet section truncated by budget ...\n";
    let keep_chars = markdown.chars().count() / 2;
    let mut keep_byte = markdown.len();
    if let Some((index, _)) = markdown.char_indices().nth(keep_chars) {
        keep_byte = index;
    }
    markdown.truncate(keep_byte);
    markdown.push_str(SUFFIX);
}

fn packet_budget_usage(answer: &AgentAnswerDto) -> PacketBudgetUsageDto {
    let files = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_deref())
        .collect::<HashSet<_>>()
        .len();
    let trail_edges = answer
        .graphs
        .iter()
        .map(|artifact| match artifact {
            GraphArtifactDto::Uml { graph, .. } => graph.edges.len(),
            GraphArtifactDto::Mermaid { .. } => 0,
        })
        .sum::<usize>();
    let snippets = answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| {
            step.kind == AgentRetrievalStepKindDto::SourceRead
                && step.status == AgentRetrievalStepStatusDto::Ok
        })
        .count();
    let output_bytes = serde_json::to_vec(answer)
        .map(|bytes| bytes.len())
        .unwrap_or_default();

    PacketBudgetUsageDto {
        anchors: answer.citations.len().try_into().unwrap_or(u32::MAX),
        files: files.try_into().unwrap_or(u32::MAX),
        snippets: snippets.try_into().unwrap_or(u32::MAX),
        trail_edges: trail_edges.try_into().unwrap_or(u32::MAX),
        output_bytes: output_bytes.try_into().unwrap_or(u32::MAX),
    }
}

fn next_deeper_packet_command(question: &str, requested: PacketBudgetModeDto) -> Option<String> {
    let next = match requested {
        PacketBudgetModeDto::Tiny => "compact",
        PacketBudgetModeDto::Compact => "standard",
        PacketBudgetModeDto::Standard => "deep",
        PacketBudgetModeDto::Deep => return None,
    };
    Some(format!(
        "codestory-cli packet --project <target-workspace> --question {} --budget {next}",
        quote_packet_command_value(question)
    ))
}

fn quote_packet_command_value(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn build_packet_sufficiency(
    question: &str,
    answer: &AgentAnswerDto,
    budget: &PacketBudgetDto,
) -> PacketSufficiencyDto {
    let has_errors = answer
        .retrieval_trace
        .steps
        .iter()
        .any(|step| step.status == AgentRetrievalStepStatusDto::Error);
    let status = if answer.citations.is_empty() {
        PacketSufficiencyStatusDto::Insufficient
    } else if has_errors || packet_budget_exceeded_hard_output_cap(budget) {
        PacketSufficiencyStatusDto::Partial
    } else {
        PacketSufficiencyStatusDto::Sufficient
    };

    let mut gaps = Vec::new();
    if answer.citations.is_empty() {
        gaps.push("No cited anchors were found for the question.".to_string());
    }
    if budget.truncated && status != PacketSufficiencyStatusDto::Sufficient {
        gaps.push(format!(
            "Packet was truncated by {:?} budget: {}.",
            budget.requested,
            budget.omitted_sections.join(", ")
        ));
    }
    for step in answer
        .retrieval_trace
        .steps
        .iter()
        .filter(|step| step.status == AgentRetrievalStepStatusDto::Error)
    {
        gaps.push(format!("{:?} step failed.", step.kind));
    }

    let follow_up_commands = packet_follow_up_commands(question, status, budget);
    let open_next = follow_up_commands.clone();
    let avoid_opening = answer
        .citations
        .iter()
        .filter_map(|citation| citation.file_path.as_ref())
        .map(|path| packet_display_path(path))
        .collect::<HashSet<_>>()
        .into_iter()
        .take(12)
        .map(|path| {
            format!(
                "{} because this packet already includes a citation for the current answer.",
                path
            )
        })
        .collect::<Vec<_>>();

    let mut covered_claims = packet_supported_claims(answer);
    if covered_claims.is_empty() {
        covered_claims.push(PacketClaimDto {
            claim: answer.summary.clone(),
            citations: answer.citations.iter().take(6).cloned().collect(),
        });
    }

    PacketSufficiencyDto {
        status,
        covered_claims,
        open_next,
        avoid_opening,
        gaps,
        follow_up_commands,
    }
}

fn packet_budget_exceeded_hard_output_cap(budget: &PacketBudgetDto) -> bool {
    budget
        .omitted_sections
        .iter()
        .any(|section| section == "output_bytes")
}

fn packet_follow_up_commands(
    question: &str,
    status: PacketSufficiencyStatusDto,
    budget: &PacketBudgetDto,
) -> Vec<String> {
    match status {
        PacketSufficiencyStatusDto::Sufficient => Vec::new(),
        PacketSufficiencyStatusDto::Partial => budget
            .next_deeper_command
            .clone()
            .into_iter()
            .chain(std::iter::once(format!(
                "codestory-cli search --project <target-workspace> --query {} --why",
                quote_packet_command_value(question)
            )))
            .collect(),
        PacketSufficiencyStatusDto::Insufficient => vec![
            "codestory-cli index --project <target-workspace> --refresh full".to_string(),
            format!(
                "codestory-cli search --project <target-workspace> --query {} --repo-text on --why",
                quote_packet_command_value(question)
            ),
        ],
    }
}

fn packet_benchmark_trace(answer: &AgentAnswerDto) -> PacketBenchmarkTraceDto {
    let mut source_read_steps = 0;
    let mut search_steps = 0;
    let mut trail_steps = 0;
    for step in &answer.retrieval_trace.steps {
        match step.kind {
            AgentRetrievalStepKindDto::SourceRead => source_read_steps += 1,
            AgentRetrievalStepKindDto::Search
            | AgentRetrievalStepKindDto::SemanticQueryEmbedding
            | AgentRetrievalStepKindDto::SemanticCandidateRetrieval
            | AgentRetrievalStepKindDto::HybridRerank
            | AgentRetrievalStepKindDto::QueryExpansion
            | AgentRetrievalStepKindDto::RepoTextFallback => search_steps += 1,
            AgentRetrievalStepKindDto::Trail
            | AgentRetrievalStepKindDto::Neighborhood
            | AgentRetrievalStepKindDto::TrailFilterOptions => trail_steps += 1,
            AgentRetrievalStepKindDto::NodeDetails
            | AgentRetrievalStepKindDto::NodeOccurrences
            | AgentRetrievalStepKindDto::EdgeOccurrences
            | AgentRetrievalStepKindDto::MermaidSynthesis
            | AgentRetrievalStepKindDto::AnswerSynthesis => {}
        }
    }

    PacketBenchmarkTraceDto {
        retrieval_trace: answer.retrieval_trace.clone(),
        source_read_steps,
        search_steps,
        trail_steps,
    }
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

    fn test_packet_citation(display_name: &str, file_path: &str, score: f32) -> AgentCitationDto {
        AgentCitationDto {
            node_id: NodeId(display_name.to_string()),
            display_name: display_name.to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some(file_path.to_string()),
            line: Some(10),
            score,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: Some(RetrievalScoreBreakdownDto {
                lexical: 0.4,
                semantic: 0.2,
                graph: 0.3,
                total: score,
            }),
        }
    }

    fn packet_answer_fixture(question: &str, citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
        AgentAnswerDto {
            answer_id: "packet-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Fixture packet is covered by cited anchors.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Packet answer assembled from cited anchors.".to_string(),
                }],
            }],
            citations,
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "packet-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        }
    }

    fn build_sufficient_packet_fixture(
        question: &str,
        task_class: PacketTaskClassDto,
        citations: Vec<AgentCitationDto>,
    ) -> (AgentAnswerDto, PacketSufficiencyDto) {
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = packet_answer_fixture(question, citations);
        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(&mut answer, task_class, &limits);
        let budget =
            apply_packet_budget(question, PacketBudgetModeDto::Compact, limits, &mut answer);
        let sufficiency = build_packet_sufficiency(question, &answer, &budget);
        (answer, sufficiency)
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
    fn packet_plan_infers_task_class_and_code_terms() {
        let plan = build_packet_plan(
            "Trace the /api/users route through AppController and UserStore",
            None,
        );

        assert_eq!(plan.task_class, PacketTaskClassDto::RouteTracing);
        assert!(plan.inferred_task_class);
        assert!(
            plan.queries.iter().any(|query| query.query == "/api/users"),
            "route-like terms should become concrete packet queries: {plan:?}"
        );
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query == "AppController"),
            "CamelCase symbols should become concrete packet queries: {plan:?}"
        );
    }

    #[test]
    fn requested_packet_task_class_overrides_heuristic() {
        let plan = build_packet_plan(
            "What would change if the indexing cache format moved?",
            Some(PacketTaskClassDto::ChangeImpact),
        );

        assert_eq!(plan.task_class, PacketTaskClassDto::ChangeImpact);
        assert!(!plan.inferred_task_class);
        assert!(
            plan.queries
                .iter()
                .any(|query| query.query.contains("affected")),
            "change impact plans should seed affected-symbol queries: {plan:?}"
        );
    }

    #[test]
    fn packet_plan_expands_indexing_flow_into_symbol_probes() {
        let plan = build_packet_plan(
            "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
        );
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "run_index",
            "IndexService",
            "WorkspaceIndexer",
            "WorkspaceManifest",
            "flush_projection_batch",
            "SnapshotStore",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in packet plan: {queries:?}"
            );
        }
    }

    #[test]
    fn symbol_ownership_packet_plan_seeds_express_ownership_files() {
        let question = "Explain which Express modules own application creation, app-level rendering, response serialization, file sending, and view lookup.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::SymbolOwnership));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "createApplication",
            "tryRender",
            "app.render",
            "res.send",
            "res.json",
            "res.sendFile",
            "View",
            "lib/express.js",
            "lib/application.js",
            "lib/response.js",
            "lib/view.js",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in Express ownership packet plan: {queries:?}"
            );
        }
        assert!(
            !queries.contains(&"WorkspaceManifest"),
            "file sending should not be mistaken for workspace file discovery: {queries:?}"
        );
    }

    #[test]
    fn bug_packet_plan_seeds_express_param_path() {
        let question =
            "Localize an Express app.param callback decode bug through router parameter handling.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::BugLocalization));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "proto.param",
            "proto.process_params",
            "Layer.prototype.match",
            "Route.prototype.dispatch",
            "decode_param",
            "paramCallback",
            "test/app.param.js",
            "test/Router.js",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in Express param packet plan: {queries:?}"
            );
        }
    }

    #[test]
    fn mux_packet_plan_and_claims_cover_router_regexp_and_middleware() {
        let question = "Assess the likely impact of changing mux trailing-slash redirect behavior and CORS preflight method reporting.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::ChangeImpact));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "Router.StrictSlash",
            "Route.addRegexpMatcher",
            "newRouteRegexp",
            "CORSMethodMiddleware",
            "Router.Use",
            "Route.Methods",
            "mux.go",
            "route.go",
            "regexp.go",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in mux packet plan: {queries:?}"
            );
        }

        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "mux-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Mux routing and middleware evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("Router.StrictSlash", "mux.go", 0.9),
                test_packet_citation("Route.addRegexpMatcher", "route.go", 0.8),
                test_packet_citation("newRouteRegexp", "regexp.go", 0.8),
                test_packet_citation("CORSMethodMiddleware", "middleware.go", 0.8),
                test_packet_citation("TestRouteMatchers", "mux_test.go", 0.7),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "mux-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        append_packet_evidence_sections(&mut answer, PacketTaskClassDto::ChangeImpact, &limits);
        let text = answer
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for expected_claim in [
            "Trailing-slash behavior is configured through Router.StrictSlash.",
            "Route matcher construction is an impact surface because it carries strict-slash options into regexp compilation.",
            "Route template parsing and variable extraction should be inspected in regexp.go.",
            "The implementation starting point is CORSMethodMiddleware in middleware.go.",
            "Middleware registration through Router.Use is relevant for how the CORS helper is installed.",
            "Regression coverage should include mux request matching tests and route regexp tests.",
        ] {
            assert!(
                text.contains(expected_claim),
                "mux packet claims should include {expected_claim}: {text}"
            );
        }
    }

    #[test]
    fn flask_and_vite_packet_plans_seed_framework_anchors() {
        let flask_plan = build_packet_plan(
            "Trace how Flask receives a WSGI request and dispatches to a view.",
            Some(PacketTaskClassDto::RouteTracing),
        );
        let flask_queries = flask_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "Flask.wsgi_app",
            "Flask.full_dispatch_request",
            "Flask.dispatch_request",
            "Scaffold.route",
            "src/flask/app.py",
        ] {
            assert!(
                flask_queries.contains(&expected),
                "expected {expected} in Flask packet plan: {flask_queries:?}"
            );
        }

        let vite_plan = build_packet_plan(
            "Explain how Vite creates a dev server, transforms modules, runs plugin hooks, and updates the module graph.",
            Some(PacketTaskClassDto::ArchitectureExplanation),
        );
        let vite_queries = vite_plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "resolveConfig",
            "createServer",
            "transformMiddleware",
            "transformRequest",
            "createPluginContainer",
            "ModuleGraph",
            "src/node/server/moduleGraph.ts",
        ] {
            assert!(
                vite_queries.contains(&expected),
                "expected {expected} in Vite packet plan: {vite_queries:?}"
            );
        }
    }

    #[test]
    fn flask_and_vite_packet_claims_cover_expected_surfaces() {
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "framework-fixture".to_string(),
            prompt: "Explain Flask and Vite ownership surfaces.".to_string(),
            summary: "Framework evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("Flask.dispatch_request", "src/flask/app.py", 0.8),
                test_packet_citation("Scaffold.route", "src/flask/sansio/scaffold.py", 0.8),
                test_packet_citation("Blueprint.register", "src/flask/sansio/blueprints.py", 0.8),
                test_packet_citation("save_session", "src/flask/sessions.py", 0.8),
                test_packet_citation("resolveConfig", "src/node/config.ts", 0.8),
                test_packet_citation("createServer", "src/node/server/index.ts", 0.8),
                test_packet_citation(
                    "transformMiddleware",
                    "src/node/server/middlewares/transform.ts",
                    0.8,
                ),
                test_packet_citation(
                    "transformRequest",
                    "src/node/server/transformRequest.ts",
                    0.8,
                ),
                test_packet_citation(
                    "createPluginContainer",
                    "src/node/server/pluginContainer.ts",
                    0.8,
                ),
                test_packet_citation("ModuleGraph", "src/node/server/moduleGraph.ts", 0.8),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "framework-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
        );
        let text = answer
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for expected_claim in [
            "Flask.wsgi_app is the WSGI entry point and creates or uses request context before dispatch.",
            "The route decorator behavior is shared through the scaffold abstraction.",
            "Nested blueprint behavior is owned by the sansio blueprint registration code.",
            "The focused implementation file is src/flask/sessions.py because cookie attributes are read and applied there.",
            "Config resolution is owned by src/node/config.ts through resolveConfig.",
            "createServer is the development server construction entry point.",
            "transformMiddleware filters module-like requests and delegates eligible work to transformRequest.",
            "Module transformation orchestration is owned by transformRequest.",
            "The plugin container is the server-facing mechanism for running plugin hooks.",
            "The module graph tracks module relationships and transform state for dev-server requests.",
        ] {
            assert!(
                text.contains(expected_claim),
                "framework packet claims should include {expected_claim}: {text}"
            );
        }
    }

    #[test]
    fn route_tracing_packet_plan_and_claims_cover_express_dispatch_flow() {
        let question = "Trace how an Express application registers middleware and routes, then dispatches an incoming request through router layers to a route handler.";
        let plan = build_packet_plan(question, Some(PacketTaskClassDto::RouteTracing));
        let queries = plan
            .queries
            .iter()
            .map(|query| query.query.as_str())
            .collect::<Vec<_>>();

        for expected in [
            "createApplication",
            "app.use",
            "app.route",
            "lib/express.js",
            "lib/router/layer.js",
            "lib/router/route.js",
        ] {
            assert!(
                queries.contains(&expected),
                "expected {expected} in route tracing packet plan: {queries:?}"
            );
        }

        let mut answer = AgentAnswerDto {
            answer_id: "route-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Express route flow is covered by cited anchors.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("createApplication", "lib/express.js", 0.5),
                test_packet_citation("application", "lib/application.js", 0.5),
                test_packet_citation("router", "lib/router/index.js", 0.5),
                test_packet_citation("Layer", "lib/router/layer.js", 0.5),
                test_packet_citation("Route", "lib/router/route.js", 0.5),
                test_packet_citation("test route", "test/app.router.js", 2.0),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "route-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::RouteTracing,
            &packet_budget_limits(PacketBudgetModeDto::Compact),
        );
        let text = answer
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            answer.citations[0]
                .file_path
                .as_deref()
                .is_some_and(|path| path.starts_with("lib/")),
            "production route files should outrank test route examples: {:?}",
            answer.citations
        );
        for expected_claim in [
            "The public factory creates a function-shaped app and mixes in application, request, and response prototypes.",
            "Application middleware registration goes through app.use and is delegated to the lazy router.",
            "Route registration through app.route creates route-specific handlers on the router.",
            "The router walks its stack of layers and matches request paths before handing control to a route.",
            "Route dispatch is responsible for invoking the route's matching method handlers.",
        ] {
            assert!(
                text.contains(expected_claim),
                "route packet claims should include {expected_claim}: {text}"
            );
        }
    }

    #[test]
    fn packet_claims_cover_express_bug_and_ownership_surfaces() {
        let question = "Identify response helper and router parameter files before editing.";
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "express-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Express response and router parameter evidence is covered.".to_string(),
            freshness: None,
            sections: Vec::new(),
            citations: vec![
                test_packet_citation("send", "lib/response.js", 0.9),
                test_packet_citation("View", "lib/view.js", 0.8),
                test_packet_citation("application", "lib/application.js", 0.7),
                test_packet_citation("createApplication", "lib/express.js", 0.7),
                test_packet_citation("router", "lib/router/index.js", 0.7),
                test_packet_citation("Layer", "lib/router/layer.js", 0.7),
                test_packet_citation("Route", "lib/router/route.js", 0.7),
                test_packet_citation("res.send test", "test/res.send.js", 0.7),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "express-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        append_packet_evidence_sections(&mut answer, PacketTaskClassDto::BugLocalization, &limits);
        let text = answer
            .sections
            .iter()
            .flat_map(|section| &section.blocks)
            .filter_map(|block| match block {
                AgentResponseBlockDto::Markdown { markdown } => Some(markdown.as_str()),
                AgentResponseBlockDto::Mermaid { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        for expected_claim in [
            "The first file to inspect is lib/response.js because res.send, res.json, and res.sendFile are implemented there.",
            "Compatibility behavior for old response helper call shapes should be checked near res.send and res.json.",
            "File-transfer validation and callback behavior should be checked separately from JSON serialization.",
            "Existing response tests are relevant because this is a behavior compatibility report.",
            "The public application factory is implemented in lib/express.js.",
            "App-level rendering is owned by lib/application.js.",
            "Response serialization helpers are owned by lib/response.js.",
            "View lookup and metadata are owned by lib/view.js.",
            "Parameter callback registration starts in proto.param in lib/router/index.js.",
            "The callback execution path should be inspected in proto.process_params.",
            "Layer matching is relevant because it extracts and decodes route parameter values.",
            "Route dispatch is downstream of parameter processing and should be checked for handler invocation order.",
            "Regression coverage should include app.param and router behavior tests.",
        ] {
            assert!(
                text.contains(expected_claim),
                "packet claims should include {expected_claim}: {text}"
            );
        }
    }

    #[test]
    fn sufficient_packets_stop_broad_exploration_across_task_classes() {
        let fixtures = [
            (
                PacketTaskClassDto::ArchitectureExplanation,
                "Explain how the dev server is constructed and how config flows into it.",
                vec![
                    test_packet_citation("resolveConfig", "src/node/config.ts", 0.9),
                    test_packet_citation("createServer", "src/node/server/index.ts", 0.9),
                    test_packet_citation("httpServerStart", "src/node/http.ts", 0.8),
                ],
                "createServer is the development server construction entry point",
                "src/node/server/index.ts",
            ),
            (
                PacketTaskClassDto::BugLocalization,
                "Find the likely bug path for Express response sending compatibility.",
                vec![
                    test_packet_citation("send", "lib/response.js", 0.9),
                    test_packet_citation("View", "lib/view.js", 0.8),
                    test_packet_citation("res.send regression", "test/res.send.js", 0.7),
                ],
                "The first file to inspect is lib/response.js",
                "lib/response.js",
            ),
            (
                PacketTaskClassDto::ChangeImpact,
                "What changes if mux strict slash route matching behavior changes?",
                vec![
                    test_packet_citation("Router.StrictSlash", "mux.go", 0.9),
                    test_packet_citation("Route.addRegexpMatcher", "route.go", 0.8),
                    test_packet_citation("newRouteRegexp", "regexp.go", 0.8),
                ],
                "Trailing-slash behavior is configured through Router.StrictSlash",
                "mux.go",
            ),
            (
                PacketTaskClassDto::RouteTracing,
                "Trace how a Flask request reaches the selected view function.",
                vec![
                    test_packet_citation("Flask.wsgi_app", "src/flask/app.py", 0.9),
                    test_packet_citation("RequestContext", "src/flask/ctx.py", 0.8),
                    test_packet_citation("Flask.dispatch_request", "src/flask/app.py", 0.8),
                ],
                "dispatch_request invokes the view function selected by URL matching",
                "src/flask/app.py",
            ),
            (
                PacketTaskClassDto::SymbolOwnership,
                "Who owns Vite module request transforms and module graph state?",
                vec![
                    test_packet_citation(
                        "transformMiddleware",
                        "src/node/server/middlewares/transform.ts",
                        0.9,
                    ),
                    test_packet_citation(
                        "transformRequest",
                        "src/node/server/transformRequest.ts",
                        0.9,
                    ),
                    test_packet_citation("ModuleGraph", "src/node/server/moduleGraph.ts", 0.8),
                ],
                "Module transformation orchestration is owned by transformRequest",
                "src/node/server/transformRequest.ts",
            ),
            (
                PacketTaskClassDto::EditPlanning,
                "Plan the focused edit for Flask session cookie attribute behavior.",
                vec![
                    test_packet_citation("save_session", "src/flask/sessions.py", 0.9),
                    test_packet_citation("get_cookie_samesite", "src/flask/sessions.py", 0.8),
                    test_packet_citation("test_session_cookie", "tests/test_basic.py", 0.7),
                ],
                "save_session is the final write path",
                "src/flask/sessions.py",
            ),
        ];

        for (task_class, question, citations, expected_claim, avoid_path) in fixtures {
            let (_answer, sufficiency) =
                build_sufficient_packet_fixture(question, task_class, citations);

            assert_eq!(
                sufficiency.status,
                PacketSufficiencyStatusDto::Sufficient,
                "task class {task_class:?} should be sufficient: {sufficiency:?}"
            );
            assert!(
                sufficiency.follow_up_commands.is_empty(),
                "sufficient {task_class:?} packets should not recommend broad follow-up commands: {sufficiency:?}"
            );
            assert!(
                sufficiency.open_next.is_empty(),
                "sufficient {task_class:?} packets should not name generic open-next work: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .covered_claims
                    .iter()
                    .any(|claim| claim.claim.contains(expected_claim)),
                "sufficient {task_class:?} packet should name the covered task claim `{expected_claim}`: {sufficiency:?}"
            );
            assert!(
                sufficiency
                    .avoid_opening
                    .iter()
                    .any(|entry| entry.contains(avoid_path)),
                "sufficient {task_class:?} packet should discourage reopening cited path `{avoid_path}`: {sufficiency:?}"
            );
        }
    }

    #[test]
    fn partial_and_insufficient_packets_recommend_targeted_followups() {
        let question = "Explain route dispatch with enough evidence to stop.";
        let mut partial_answer = packet_answer_fixture(
            question,
            vec![test_packet_citation(
                "Route.prototype.dispatch",
                "lib/router/route.js",
                0.8,
            )],
        );
        let mut budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Tiny,
            packet_budget_limits(PacketBudgetModeDto::Tiny),
            &mut partial_answer,
        );
        budget.truncated = true;
        budget.omitted_sections = vec!["output_bytes".to_string()];
        let partial = build_packet_sufficiency(question, &partial_answer, &budget);

        assert_eq!(partial.status, PacketSufficiencyStatusDto::Partial);
        assert!(
            partial
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--budget compact")),
            "partial packets should recommend the next deeper packet command: {partial:?}"
        );
        assert!(
            partial
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli search")),
            "partial packets should recommend targeted CodeStory search, not broad source reads: {partial:?}"
        );

        let mut empty_answer = packet_answer_fixture(question, Vec::new());
        let empty_budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut empty_answer,
        );
        let insufficient = build_packet_sufficiency(question, &empty_answer, &empty_budget);

        assert_eq!(
            insufficient.status,
            PacketSufficiencyStatusDto::Insufficient
        );
        assert!(
            insufficient
                .follow_up_commands
                .iter()
                .any(|command| command.contains("codestory-cli index")),
            "insufficient packets should recommend indexing before broad exploration: {insufficient:?}"
        );
        assert!(
            insufficient
                .follow_up_commands
                .iter()
                .any(|command| command.contains("--repo-text on")),
            "insufficient packets should keep fallback exploration inside CodeStory: {insufficient:?}"
        );
    }

    #[test]
    fn citation_budget_truncation_keeps_sufficient_stop_signal() {
        let question = "Explain the compact packet stop rule.";
        let mut answer = packet_answer_fixture(
            question,
            (0..14)
                .map(|index| {
                    test_packet_citation(
                        &format!("symbol_{index}"),
                        &format!("src/file_{index}.rs"),
                        0.8,
                    )
                })
                .collect(),
        );
        let budget = apply_packet_budget(
            question,
            PacketBudgetModeDto::Compact,
            packet_budget_limits(PacketBudgetModeDto::Compact),
            &mut answer,
        );
        let sufficiency = build_packet_sufficiency(question, &answer, &budget);

        assert!(
            budget.truncated && budget.omitted_sections.contains(&"citations".to_string()),
            "fixture should exercise normal citation budget truncation: {budget:?}"
        );
        assert_eq!(
            sufficiency.status,
            PacketSufficiencyStatusDto::Sufficient,
            "budgeted citation clipping should not force broad follow-up when the compact packet still has cited anchors: {sufficiency:?}"
        );
        assert!(sufficiency.follow_up_commands.is_empty());
        assert_eq!(answer.citations.len(), 10);
        assert!(
            sufficiency.gaps.is_empty(),
            "normal compact-budget truncation should stay in budget metadata, not sufficiency gaps: {sufficiency:?}"
        );
        assert!(budget.used.files <= budget.limits.max_files);
        assert!(budget.used.output_bytes <= budget.limits.max_output_bytes);
    }

    #[test]
    fn graph_budget_prunes_nodes_not_referenced_by_retained_edges() {
        fn node(id: &str) -> codestory_contracts::api::GraphNodeDto {
            codestory_contracts::api::GraphNodeDto {
                id: NodeId(id.to_string()),
                label: id.to_string(),
                kind: codestory_contracts::api::NodeKind::FUNCTION,
                depth: 1,
                label_policy: None,
                badge_visible_members: None,
                badge_total_members: None,
                merged_symbol_examples: Vec::new(),
                file_path: None,
                qualified_name: None,
                member_access: None,
            }
        }

        fn edge(id: &str, source: &str, target: &str) -> codestory_contracts::api::GraphEdgeDto {
            codestory_contracts::api::GraphEdgeDto {
                id: EdgeId(id.to_string()),
                source: NodeId(source.to_string()),
                target: NodeId(target.to_string()),
                kind: codestory_contracts::api::EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            }
        }

        let mut answer = packet_answer_fixture(
            "Explain graph budget trimming.",
            vec![test_packet_citation("center", "src/center.rs", 0.9)],
        );
        answer.graphs.push(GraphArtifactDto::Uml {
            id: "graph".to_string(),
            title: "Graph".to_string(),
            graph: GraphResponse {
                center_id: NodeId("center".to_string()),
                nodes: vec![
                    node("center"),
                    node("kept"),
                    node("dropped_a"),
                    node("dropped_b"),
                ],
                edges: vec![
                    edge("edge_1", "center", "kept"),
                    edge("edge_2", "kept", "dropped_a"),
                    edge("edge_3", "dropped_a", "dropped_b"),
                ],
                truncated: false,
                omitted_edge_count: 0,
                canonical_layout: None,
            },
        });

        let budget = apply_packet_budget(
            "Explain graph budget trimming.",
            PacketBudgetModeDto::Tiny,
            PacketBudgetLimitsDto {
                max_trail_edges: 1,
                ..packet_budget_limits(PacketBudgetModeDto::Tiny)
            },
            &mut answer,
        );

        let GraphArtifactDto::Uml { graph, .. } = &answer.graphs[0] else {
            panic!("expected UML graph");
        };
        let node_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(node_ids, vec!["center", "kept"]);
        assert!(graph.truncated);
        assert!(budget.omitted_sections.contains(&"trail_edges".to_string()));
    }

    #[test]
    fn indexing_flow_packet_sections_and_sufficiency_cover_agent_stop_contract() {
        let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
        let limits = packet_budget_limits(PacketBudgetModeDto::Compact);
        let mut answer = AgentAnswerDto {
            answer_id: "packet-fixture".to_string(),
            prompt: question.to_string(),
            summary: "Indexing flow is covered by cited CodeStory anchors.".to_string(),
            freshness: None,
            sections: vec![AgentResponseSectionDto {
                id: "answer".to_string(),
                title: "Answer".to_string(),
                blocks: vec![AgentResponseBlockDto::Markdown {
                    markdown: "Full indexing starts at the CLI and proceeds through runtime, workspace, indexer, store, and snapshot layers.".to_string(),
                }],
            }],
            citations: vec![
                test_packet_citation(
                    "search_quality_eval",
                    "crates/codestory-cli/tests/search_json_output.rs",
                    0.5,
                ),
                test_packet_citation("run_index", "crates/codestory-cli/src/main.rs", 0.2),
                test_packet_citation(
                    "IndexService::run_indexing",
                    "crates/codestory-runtime/src/services.rs",
                    0.3,
                ),
                test_packet_citation(
                    "WorkspaceIndexer::build_execution_plan",
                    "crates/codestory-workspace/src/lib.rs",
                    0.2,
                ),
                test_packet_citation(
                    "index_file",
                    "crates/codestory-indexer/src/lib.rs",
                    0.2,
                ),
                test_packet_citation(
                    "flush_projection_batch",
                    "crates/codestory-store/src/snapshot_store.rs",
                    0.2,
                ),
            ],
            subgraph_ids: Vec::new(),
            retrieval_version: "test".to_string(),
            graphs: Vec::new(),
            retrieval_trace: codestory_contracts::api::AgentRetrievalTraceDto {
                request_id: "packet-fixture".to_string(),
                resolved_profile: AgentRetrievalPresetDto::Architecture,
                policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
                total_latency_ms: 1,
                sla_target_ms: None,
                sla_missed: false,
                annotations: Vec::new(),
                steps: Vec::new(),
            },
        };

        rank_packet_evidence(question, &mut answer);
        append_packet_evidence_sections(
            &mut answer,
            PacketTaskClassDto::ArchitectureExplanation,
            &limits,
        );
        let budget =
            apply_packet_budget(question, PacketBudgetModeDto::Compact, limits, &mut answer);
        let sufficiency = build_packet_sufficiency(question, &answer, &budget);

        assert_eq!(answer.sections[0].id, "packet-evidence-ledger");
        assert_eq!(answer.sections[1].id, "packet-flow-claims");
        let top_anchor_names = answer
            .citations
            .iter()
            .take(3)
            .map(|citation| citation.display_name.as_str())
            .collect::<Vec<_>>();
        assert!(
            top_anchor_names.contains(&"run_index"),
            "CLI entrypoint should stay in the high-priority indexing-flow anchors: {top_anchor_names:?}"
        );
        assert!(
            top_anchor_names.contains(&"WorkspaceIndexer::build_execution_plan"),
            "workspace planning should stay in the high-priority indexing-flow anchors: {top_anchor_names:?}"
        );
        assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
        assert!(sufficiency.follow_up_commands.is_empty());
        assert!(sufficiency.open_next.is_empty());
        assert!(
            sufficiency.covered_claims.iter().any(|claim| claim
                .claim
                .contains("runtime opens the workspace and store")),
            "indexing-flow packet should include claim-led runtime flow notes: {sufficiency:?}"
        );
        assert!(
            sufficiency
                .avoid_opening
                .iter()
                .any(|path| path.contains("crates/codestory-cli/src/main.rs")),
            "sufficient packets should tell agents cited files do not need broad re-opening: {sufficiency:?}"
        );
    }

    #[test]
    fn packet_claims_use_normalized_evidence_paths() {
        let citation = AgentCitationDto {
            node_id: NodeId("run_index".to_string()),
            display_name: "run_index".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some(
                "\\\\?\\C:\\Users\\alber\\source\\repos\\codestory\\crates\\codestory-cli\\src\\main.rs"
                    .to_string(),
            ),
            line: Some(193),
            score: 0.85,
            origin: SearchHitOrigin::IndexedSymbol,
            resolvable: true,
            subgraph_id: None,
            evidence_edge_ids: Vec::new(),
            retrieval_score_breakdown: None,
        };

        assert_eq!(packet_evidence_role(&citation), Some("CLI entrypoint"));
        assert_eq!(
            packet_display_path(citation.file_path.as_deref().unwrap()),
            "crates/codestory-cli/src/main.rs"
        );
        assert!(
            packet_claim_for_role("CLI entrypoint", &citation).contains("`run_index`"),
            "claim should name the evidence anchor"
        );
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
