use super::*;

pub(super) fn run_drill(cmd: DrillCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill")?;
    let operation = execute_drill(&cmd)?;
    let contents = write_drill_outputs(cmd.format, &cmd.output_dir, &operation)?;
    print!("{}", contents.selected);
    Ok(())
}

pub(super) fn execute_drill(
    cmd: &DrillCommand,
) -> Result<codestory_runtime::PublicOperation<DrillOutput>> {
    let _ = cmd.jobs; // retained CLI compatibility; packet owns internal batch scheduling
    let total_timer = Instant::now();
    let setup_timer = Instant::now();
    validate_drill_output_dir(&cmd.output_dir)?;
    let OpenedAgentSurface {
        runtime,
        before,
        opened,
    } = open_agent_surface(
        &cmd.project,
        cmd.profile,
        cmd.run_id.as_deref(),
        cmd.refresh,
        "drill",
    )?;
    if cmd.refresh != args::RefreshMode::None {
        retrieval::finalize_retrieval_index_for_runtime(&runtime)
            .context("drill retrieval index finalize")?;
    }
    let refresh = refresh_label(cmd.refresh, opened.refresh_mode);
    let before_stats = before.as_ref().map(|summary| &summary.stats);
    let before_unavailable_reason = before.is_none().then(|| {
        opened
            .refresh_reason
            .clone()
            .unwrap_or_else(|| "pre_refresh_summary_unavailable".to_string())
    });
    let setup_ms = elapsed_ms(setup_timer);

    let drill_anchors = drill_targeting::validated_drill_anchors(&cmd.anchors, "drill")?;
    let question = cmd
        .question
        .clone()
        .unwrap_or_else(|| format!("Investigate anchors: {}", drill_anchors.join(", ")));
    let packet_timer = Instant::now();
    let packet_request = AgentPacketRequestDto {
        question,
        budget: PacketBudgetModeDto::Standard,
        task_class: None,
        probes: Vec::new(),
        extra_probes: drill_anchors.clone(),
        include_evidence: true,
        latency_budget_ms: None,
    };
    runtime.run_public_operation("drill", || {
        let pinned_summary = runtime.active_project_summary()?;
        let pinned_publication = runtime
            .public_operation
            .active_publication()
            .context("drill public operation has active publication identity")?;
        let sidecar_retrieval_mode = pinned_publication
            .retrieval_publication
            .as_ref()
            .map(|_| "full".to_string());
        let evidence_packet = execute_drill_packet(packet_request.clone(), |request| {
            runtime.browser.packet(request)
        })?;
        let question_search_ms = elapsed_ms(packet_timer);
        let evidence_assembly_timer = Instant::now();
        let citations = drill_packet_citations(&evidence_packet);
        let anchor_outputs =
            drill_packet_anchors(&runtime.project_root, &drill_anchors, &citations);
        let bridge_outputs = drill_packet_bridges(&runtime.project_root, &evidence_packet);
        let mut all_verification_targets =
            drill_packet_verification_targets(&runtime.project_root, &citations);
        dedupe_verification_targets(&mut all_verification_targets);
        let next_commands = evidence_packet.sufficiency.follow_up_commands.clone();
        let question_search = Some(DrillCommandStatusOutput {
            command: "packet".to_string(),
            status: packet_sufficiency_label(evidence_packet.sufficiency.status).to_string(),
            duration_ms: u64::from(evidence_packet.answer.retrieval_trace.total_latency_ms),
            artifact: None,
            error: None,
        });
        let evidence_assembly_ms = elapsed_ms(evidence_assembly_timer);
        let drill_timings = DrillRuntimeTimingsOutput {
            total_ms: elapsed_ms(total_timer),
            setup_ms,
            question_search_ms,
            anchor_resolution_ms: 0,
            supplemental_search_ms: 0,
            bridge_evidence_ms: 0,
            evidence_assembly_ms,
        };

        Ok(DrillOutput {
            project: display::clean_path_string(&pinned_summary.root),
            label: cmd.label.clone(),
            question: cmd.question.clone(),
            output_dir: display::clean_path_string(&cmd.output_dir.to_string_lossy()),
            mechanical: DrillMechanicalOutput {
                before_files: before_stats.map(|stats| stats.file_count),
                before_nodes: before_stats.map(|stats| stats.node_count),
                before_edges: before_stats.map(|stats| stats.edge_count),
                before_errors: before_stats.map(|stats| stats.error_count),
                before_unavailable_reason: before_unavailable_reason.clone(),
                after_files: pinned_summary.stats.file_count,
                after_nodes: pinned_summary.stats.node_count,
                after_edges: pinned_summary.stats.edge_count,
                after_errors: pinned_summary.stats.error_count,
                refresh: refresh.clone(),
                retrieval: pinned_summary.retrieval.clone(),
                sidecar_retrieval_mode,
                freshness: pinned_summary.freshness.clone(),
                phase_timings: opened.phase_timings.clone(),
                drill_timings,
            },
            question_search,
            question_supplemental_searches: Vec::new(),
            anchors: anchor_outputs,
            bridges: bridge_outputs,
            execution_boundaries: vec![DrillExecutionBoundaryOutput {
                command: "packet".to_string(),
                flow: vec![
                    "plan question and explicit anchor probes".to_string(),
                    "execute one bounded batch retrieval".to_string(),
                    "adapt citations and sufficiency into drill reports".to_string(),
                ],
                source_files: vec![
                    "crates/codestory-runtime/src/agent/orchestrator.rs".to_string(),
                    "crates/codestory-runtime/src/agent/packet_batch.rs".to_string(),
                ],
            }],
            verification_targets: all_verification_targets,
            evidence_packet,
            next_commands,
        })
    })
}

pub(super) fn execute_drill_packet(
    request: AgentPacketRequestDto,
    execute: impl FnOnce(AgentPacketRequestDto) -> Result<AgentPacketDto, ApiError>,
) -> Result<AgentPacketDto> {
    execute(request).map_err(map_api_error)
}

pub(super) fn drill_packet_citations(packet: &AgentPacketDto) -> Vec<AgentCitationDto> {
    let mut citations = packet.answer.citations.clone();
    for claim in &packet.sufficiency.covered_claims {
        citations.extend(claim.citations.iter().cloned());
    }
    let mut seen = HashSet::new();
    citations.retain(|citation| {
        seen.insert((
            citation.node_id.0.clone(),
            citation.file_path.clone(),
            citation.line,
        ))
    });
    citations
}

pub(super) fn drill_packet_anchors(
    project_root: &std::path::Path,
    anchors: &[String],
    citations: &[AgentCitationDto],
) -> Vec<DrillAnchorOutput> {
    anchors
        .iter()
        .map(|anchor| {
            let normalized = codestory_runtime::normalize_symbol_query(anchor);
            let citation = citations
                .iter()
                .filter(|citation| drill_packet_citation_is_typed_resolvable(citation))
                .filter(|citation| {
                    let display = codestory_runtime::normalize_symbol_query(&citation.display_name);
                    display == normalized
                        || codestory_runtime::terminal_symbol_segment(&citation.display_name)
                            == normalized
                })
                .max_by(|left, right| left.score.total_cmp(&right.score));
            let chosen_anchor = citation.map(|citation| {
                drill_search_hit_from_packet_citation(project_root, anchor, citation)
            });
            let verification_targets = citation
                .and_then(|citation| drill_packet_verification_target(project_root, citation))
                .into_iter()
                .collect();
            DrillAnchorOutput {
                anchor: anchor.clone(),
                typed_hit_count: usize::from(citation.is_some()),
                chosen_anchor,
                verification_targets,
                consumer_summary: None,
                timings: DrillAnchorTimingsOutput::default(),
                commands: Vec::new(),
            }
        })
        .collect()
}

pub(super) fn drill_search_hit_from_packet_citation(
    project_root: &std::path::Path,
    query: &str,
    citation: &AgentCitationDto,
) -> SearchHitOutput {
    let file_path = citation
        .file_path
        .as_deref()
        .map(|path| display::relative_path(project_root, path));
    let match_quality = if codestory_runtime::normalize_symbol_query(query)
        == codestory_runtime::normalize_symbol_query(&citation.display_name)
    {
        SearchMatchQualityDto::NormalizedExact
    } else {
        SearchMatchQualityDto::SemanticSuggestion
    };
    let verification_targets = drill_packet_verification_target(project_root, citation)
        .into_iter()
        .collect();
    SearchHitOutput {
        number: None,
        node_id: citation.node_id.0.clone(),
        node_ref: crate::output::node_ref(
            project_root,
            citation.file_path.as_deref(),
            citation.line,
            &citation.display_name,
        ),
        display_name: citation.display_name.clone(),
        kind: citation.kind,
        file_path,
        line: citation.line,
        score: citation.score,
        origin: citation.origin,
        match_quality,
        resolvable: citation.resolvable,
        evidence_tier: citation.evidence_tier,
        evidence_producer: citation.evidence_producer.clone(),
        resolution_status: citation.resolution_status,
        eligible_for_sufficiency: citation.eligible_for_sufficiency,
        score_breakdown: citation.retrieval_score_breakdown.clone(),
        duplicate_of: None,
        excerpt: None,
        primary_occurrence_kind: None,
        symbol_role: citation.coverage_role.clone(),
        paired_refs: Vec::new(),
        verification_targets,
        resolution_hints: Vec::new(),
        why: citation
            .evidence_producer
            .iter()
            .map(|producer| format!("packet evidence producer: {producer}"))
            .collect(),
    }
}

pub(super) fn drill_packet_verification_target(
    project_root: &std::path::Path,
    citation: &AgentCitationDto,
) -> Option<VerificationTargetOutput> {
    if !drill_packet_citation_is_typed_resolvable(citation) {
        return None;
    }
    Some(VerificationTargetOutput {
        role: citation
            .coverage_role
            .clone()
            .unwrap_or_else(|| "packet citation".to_string()),
        path: display::relative_path(project_root, citation.file_path.as_deref()?),
        line: citation.line.unwrap_or(1),
        node_ref: None,
        reason: format!("packet citation for {}", citation.display_name),
    })
}

pub(super) fn drill_packet_citation_is_typed_resolvable(citation: &AgentCitationDto) -> bool {
    citation.resolvable
        && citation.kind != NodeKind::UNKNOWN
        && citation.evidence_tier
            != Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
        && citation.resolution_status
            != Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
}

pub(super) fn drill_packet_verification_targets(
    project_root: &std::path::Path,
    citations: &[AgentCitationDto],
) -> Vec<VerificationTargetOutput> {
    citations
        .iter()
        .filter_map(|citation| drill_packet_verification_target(project_root, citation))
        .collect()
}

pub(super) fn drill_packet_bridges(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
) -> Vec<DrillBridgeOutput> {
    packet
        .sufficiency
        .covered_claims
        .iter()
        .filter_map(|claim| {
            let from = claim
                .citations
                .iter()
                .find(|citation| drill_packet_citation_is_typed_resolvable(citation))?;
            let to = claim.citations.iter().find(|citation| {
                citation.node_id != from.node_id
                    && drill_packet_citation_is_typed_resolvable(citation)
            })?;
            let graph_backed = drill_packet_citations_share_graph_evidence(from, to);
            let mut endpoint_files = [from.file_path.clone(), to.file_path.clone()]
                .into_iter()
                .flatten()
                .map(|path| display::relative_path(project_root, &path))
                .collect::<Vec<_>>();
            endpoint_files.sort();
            endpoint_files.dedup();
            Some(DrillBridgeOutput {
                evidence: DrillBridgeEvidenceOutput {
                    from_anchor: from.display_name.clone(),
                    to_anchor: to.display_name.clone(),
                    status: if graph_backed {
                        "graph_path".to_string()
                    } else {
                        "source_truth_only".to_string()
                    },
                    strategy: "packet_claim".to_string(),
                    confidence: match claim.proof_status {
                        Some(PacketProofStatusDto::Proven) => "high",
                        Some(PacketProofStatusDto::Likely) => "medium",
                        _ => "low",
                    }
                    .to_string(),
                    evidence_kind: "packet_citations".to_string(),
                    from_node: Some(drill_search_hit_from_packet_citation(
                        project_root,
                        &from.display_name,
                        from,
                    )),
                    to_node: Some(drill_search_hit_from_packet_citation(
                        project_root,
                        &to.display_name,
                        to,
                    )),
                    graph_path: None,
                    shared_files: Vec::new(),
                    endpoint_files: endpoint_files.clone(),
                    evidence_files: endpoint_files,
                    next_commands: packet.sufficiency.follow_up_commands.clone(),
                    notes: vec![claim.claim.clone()],
                },
                command: DrillCommandStatusOutput {
                    command: "packet".to_string(),
                    status: packet_sufficiency_label(packet.sufficiency.status).to_string(),
                    duration_ms: 0,
                    artifact: None,
                    error: None,
                },
            })
        })
        .collect()
}

pub(super) fn drill_packet_citations_share_graph_evidence(
    from: &AgentCitationDto,
    to: &AgentCitationDto,
) -> bool {
    from.evidence_edge_ids
        .iter()
        .any(|edge| to.evidence_edge_ids.contains(edge))
}

pub(super) fn write_drill_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    operation: &codestory_runtime::PublicOperation<DrillOutput>,
) -> Result<DrillReportContents> {
    let output = &operation.value;
    let report_ext = match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let markdown = render_drill_markdown(output);
    let contents = render_drill_contents(format, operation, &markdown)?;
    let report_path = output_dir.join(format!("drill-report.{report_ext}"));
    write_drill_report_file(&report_path, &contents.selected)?;
    let markdown_path = output_dir.join("drill-report.md");
    if report_path != markdown_path {
        write_drill_report_file(&markdown_path, &contents.markdown)?;
    }
    let json_path = output_dir.join("drill-report.json");
    if report_path != json_path {
        write_drill_report_file(&json_path, &contents.json)?;
    }
    let summary = drill_summary(output);
    let summary = runtime::public_operation_json_value(operation, &summary)?;
    let summary_json = ensure_trailing_newline(
        serde_json::to_string_pretty(&summary).context("Failed to serialize drill summary JSON")?,
    );
    write_drill_report_file(&output_dir.join("drill-summary.json"), &summary_json)?;
    Ok(contents)
}

pub(super) fn run_drill_suite(cmd: DrillSuiteCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "drill-suite")?;
    validate_drill_output_dir(&cmd.output_dir)?;
    let suite_output = execute_codestory_real_repo_drill_suite(&cmd)?;
    emit_drill_suite_progress(format!(
        "writing suite reports output_dir={}",
        display::clean_path_string(&cmd.output_dir.to_string_lossy())
    ));
    write_drill_suite_outputs(cmd.format, &cmd.output_dir, &suite_output)?;
    emit_drill_suite_progress(format!(
        "done repos={} ready={} degraded={} blocked={} output_dir={}",
        suite_output.repo_count,
        suite_output.ready_count,
        suite_output.degraded_count,
        suite_output.blocked_count,
        suite_output.output_dir
    ));
    let markdown = render_drill_suite_markdown(&suite_output);
    let selected = match cmd.format {
        args::OutputFormat::Markdown => ensure_trailing_newline(markdown),
        args::OutputFormat::Json => ensure_trailing_newline(
            serde_json::to_string_pretty(&suite_output)
                .context("Failed to serialize drill suite JSON")?,
        ),
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    print!("{selected}");
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(super) struct DrillSuiteCaseManifest {
    #[serde(default)]
    suite: Option<String>,
    cases: Vec<DrillSuiteCaseConfig>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DrillSuiteCaseConfig {
    slug: String,
    project: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    #[serde(default)]
    expect: DrillSuiteCaseExpectConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct DrillSuiteCaseExpectConfig {
    #[serde(default)]
    source_truth_files: Vec<String>,
    #[serde(default)]
    false_claims: Vec<String>,
    #[serde(default)]
    min_anchor_resolution: Option<usize>,
    #[serde(default)]
    allow_partial_bridges: Option<bool>,
}

#[derive(Debug)]
pub(super) struct DrillSuiteCase {
    slug: String,
    project_root: std::path::PathBuf,
    question: String,
    anchors: Vec<String>,
    expectations: DrillSuiteExpectationOutput,
}

pub(super) fn emit_drill_suite_progress(message: impl AsRef<str>) {
    eprintln!("[drill-suite] {}", message.as_ref());
}

pub(super) fn drill_suite_repo_progress_start_message(
    index: usize,
    total: usize,
    case: &DrillSuiteCase,
    repo_output_dir: &std::path::Path,
) -> String {
    format!(
        "[{index}/{total}] start {} project={} output_dir={}",
        case.slug,
        display::clean_path_string(&case.project_root.to_string_lossy()),
        display::clean_path_string(&repo_output_dir.to_string_lossy())
    )
}

pub(super) fn drill_suite_repo_progress_done_message(
    index: usize,
    total: usize,
    slug: &str,
    summary: &DrillSummaryOutput,
) -> String {
    format!(
        "[{index}/{total}] done {slug} verdict={} anchors={}/{} bridges=graph:{} partial:{} unresolved:{} output_dir={}",
        summary.verdict.status,
        summary.anchors.resolved,
        summary.anchors.requested,
        summary.bridges.graph_path,
        summary.bridges.partial,
        summary.bridges.unresolved_or_error,
        summary.output_dir
    )
}

pub(super) fn execute_codestory_real_repo_drill_suite(
    cmd: &DrillSuiteCommand,
) -> Result<DrillSuiteOutput> {
    let owner_root = cmd
        .project
        .project
        .canonicalize()
        .with_context(|| format!("Failed to resolve {}", cmd.project.project.display()))?;
    let (suite_name, cases) = drill_suite_cases_from_manifest(&cmd.case_file, &owner_root)?;
    let total_cases = cases.len();
    emit_drill_suite_progress(format!(
        "start cases={} refresh={} output_dir={}",
        total_cases,
        format!("{:?}", cmd.refresh).to_ascii_lowercase(),
        display::clean_path_string(&cmd.output_dir.to_string_lossy())
    ));
    let suite_jobs = drill_suite_case_jobs(cmd.jobs, cmd.refresh, total_cases);
    let drill_jobs = if suite_jobs > 1 {
        1
    } else {
        drill_read_only_jobs(cmd.jobs, cmd.refresh)
    };
    let repos = run_drill_suite_cases(cmd, cases, suite_jobs, drill_jobs);

    let degraded_count = drill_suite_verdict_count(&repos, "degraded");
    let blocked_count = drill_suite_verdict_count(&repos, "blocked");
    let ready_count = drill_suite_verdict_count(&repos, "ready");
    let next_actions = repos
        .iter()
        .map(|repo| format!("{}: {}", repo.slug, repo.summary.verdict.next_action))
        .collect::<Vec<_>>();
    let retrieval_blockers = drill_suite_retrieval_blockers(&repos);

    Ok(DrillSuiteOutput {
        suite: suite_name,
        project: display::clean_path_string(&owner_root.to_string_lossy()),
        case_file: display::clean_path_string(&cmd.case_file.to_string_lossy()),
        output_dir: display::clean_path_string(&cmd.output_dir.to_string_lossy()),
        repo_count: repos.len(),
        degraded_count,
        blocked_count,
        ready_count,
        repos,
        retrieval_blockers,
        next_actions,
    })
}

pub(super) fn drill_suite_case_jobs(
    requested: usize,
    refresh: args::RefreshMode,
    total_cases: usize,
) -> usize {
    if total_cases <= 1 {
        1
    } else {
        drill_read_only_jobs(requested, refresh).min(total_cases)
    }
}

pub(super) fn run_drill_suite_cases(
    cmd: &DrillSuiteCommand,
    cases: Vec<DrillSuiteCase>,
    jobs: usize,
    drill_jobs: usize,
) -> Vec<DrillSuiteRepoOutput> {
    let total_cases = cases.len();
    if jobs <= 1 || total_cases <= 1 {
        return cases
            .iter()
            .enumerate()
            .map(|(case_index, case)| {
                run_drill_suite_case(cmd, case_index, total_cases, case, drill_jobs)
            })
            .collect();
    }

    let indexed_cases = cases.into_iter().enumerate().collect::<Vec<_>>();
    let chunk_size = indexed_cases.len().div_ceil(jobs);
    let mut repos_by_case = vec![None; total_cases];
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in indexed_cases.chunks(chunk_size) {
            handles.push(scope.spawn(move || {
                chunk
                    .iter()
                    .map(|(case_index, case)| {
                        let repo = run_drill_suite_case(cmd, *case_index, total_cases, case, 1);
                        (*case_index, repo)
                    })
                    .collect::<Vec<_>>()
            }));
        }

        for handle in handles {
            for (case_index, repo) in handle.join().expect("drill-suite worker panicked") {
                repos_by_case[case_index] = Some(repo);
            }
        }
    });

    repos_by_case
        .into_iter()
        .map(|repo| repo.expect("drill-suite worker should fill every case"))
        .collect()
}

pub(super) fn run_drill_suite_case(
    cmd: &DrillSuiteCommand,
    case_index: usize,
    total_cases: usize,
    case: &DrillSuiteCase,
    drill_jobs: usize,
) -> DrillSuiteRepoOutput {
    let progress_index = case_index + 1;
    let repo_output_dir = cmd.output_dir.join(format!("{}-drill", case.slug));
    emit_drill_suite_progress(drill_suite_repo_progress_start_message(
        progress_index,
        total_cases,
        case,
        &repo_output_dir,
    ));
    let drill_cmd = DrillCommand {
        project: ProjectArgs {
            project: case.project_root.clone(),
            cache_dir: drill_suite_case_cache_dir(cmd.project.cache_dir.as_deref(), &case.slug),
        },
        anchors: case
            .anchors
            .iter()
            .map(|anchor| anchor.to_string())
            .collect(),
        label: Some(case.slug.clone()),
        question: Some(case.question.clone()),
        output_dir: repo_output_dir.clone(),
        refresh: cmd.refresh,
        profile: None,
        run_id: None,
        format: cmd.format,
        jobs: drill_jobs,
    };
    match execute_drill(&drill_cmd).and_then(|operation| {
        write_drill_outputs(cmd.format, &repo_output_dir, &operation)?;
        Ok(drill_summary(&operation.value))
    }) {
        Ok(summary) => {
            emit_drill_suite_progress(drill_suite_repo_progress_done_message(
                progress_index,
                total_cases,
                &case.slug,
                &summary,
            ));
            DrillSuiteRepoOutput {
                slug: case.slug.clone(),
                project: display::clean_path_string(&case.project_root.to_string_lossy()),
                question: case.question.clone(),
                anchors: case.anchors.clone(),
                output_dir: display::clean_path_string(&repo_output_dir.to_string_lossy()),
                artifact_extension: drill_artifact_extension(cmd.format).to_string(),
                summary,
                expectations: case.expectations.clone(),
            }
        }
        Err(error) => {
            emit_drill_suite_progress(format!(
                "[{progress_index}/{total_cases}] blocked {} error={}",
                case.slug, error
            ));
            blocked_drill_suite_repo_output(
                case,
                &repo_output_dir,
                cmd.refresh,
                cmd.format,
                &error.to_string(),
            )
        }
    }
}

pub(super) fn drill_suite_verdict_count(repos: &[DrillSuiteRepoOutput], status: &str) -> usize {
    repos
        .iter()
        .filter(|repo| repo.summary.verdict.status == status)
        .count()
}

pub(super) fn drill_suite_case_cache_dir(
    suite_cache_dir: Option<&std::path::Path>,
    slug: &str,
) -> Option<std::path::PathBuf> {
    suite_cache_dir.map(|cache_dir| cache_dir.join(output_slug(slug)))
}

pub(super) fn drill_suite_cases_from_manifest(
    case_file: &std::path::Path,
    owner_root: &std::path::Path,
) -> Result<(String, Vec<DrillSuiteCase>)> {
    let case_file = absolute_existing_path(case_file).with_context(|| {
        format!(
            "Failed to resolve drill-suite case file {}",
            display::clean_path_string(&case_file.to_string_lossy())
        )
    })?;
    let manifest_text = fs::read_to_string(&case_file).with_context(|| {
        format!(
            "Failed to read drill-suite case file {}",
            display::clean_path_string(&case_file.to_string_lossy())
        )
    })?;
    let manifest: DrillSuiteCaseManifest =
        serde_json::from_str(&manifest_text).with_context(|| {
            format!(
                "Failed to parse drill-suite case file {} as JSON",
                display::clean_path_string(&case_file.to_string_lossy())
            )
        })?;
    if manifest.cases.is_empty() {
        bail!("drill-suite case file must contain at least one case");
    }
    let manifest_dir = case_file
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(owner_root);
    let mut cases = Vec::with_capacity(manifest.cases.len());
    let mut seen_slugs = HashSet::new();
    for case in manifest.cases {
        let slug = output_slug(&case.slug);
        if slug.is_empty() {
            bail!("drill-suite case slug cannot be empty");
        }
        if !seen_slugs.insert(slug.clone()) {
            bail!("drill-suite case slug `{slug}` is duplicated");
        }
        if case.question.trim().is_empty() {
            bail!("drill-suite case `{slug}` question cannot be empty");
        }
        let anchors = drill_targeting::validated_drill_anchors(
            &case.anchors,
            &format!("drill-suite case `{slug}`"),
        )?;
        let project_root = if case.project.is_absolute() {
            case.project
        } else {
            manifest_dir.join(case.project)
        };
        cases.push(DrillSuiteCase {
            slug,
            project_root,
            question: case.question,
            anchors,
            expectations: drill_suite_expectations_from_config(case.expect),
        });
    }
    Ok((
        manifest
            .suite
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "codestory-agent-drill-suite".to_string()),
        cases,
    ))
}

pub(super) fn drill_suite_expectations_from_config(
    config: DrillSuiteCaseExpectConfig,
) -> DrillSuiteExpectationOutput {
    let mut source_truth_files = config
        .source_truth_files
        .into_iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut source_truth_files);
    let mut false_claims = config
        .false_claims
        .into_iter()
        .map(|claim| claim.trim().to_string())
        .filter(|claim| !claim.is_empty())
        .collect::<Vec<_>>();
    false_claims.sort_by_key(|claim| drill_suite_text_key(claim));
    false_claims.dedup_by(|left, right| drill_suite_text_key(left) == drill_suite_text_key(right));
    DrillSuiteExpectationOutput {
        source_truth_files,
        false_claims,
        min_anchor_resolution: config.min_anchor_resolution,
        allow_partial_bridges: config.allow_partial_bridges,
    }
}

pub(super) fn absolute_existing_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to resolve current working directory")?
            .join(path)
    };
    fs::metadata(&path).with_context(|| {
        format!(
            "Failed to access path {}",
            display::clean_path_string(&path.to_string_lossy())
        )
    })?;
    Ok(path)
}

pub(super) fn blocked_drill_suite_repo_output(
    case: &DrillSuiteCase,
    repo_output_dir: &std::path::Path,
    refresh: args::RefreshMode,
    format: args::OutputFormat,
    error: &str,
) -> DrillSuiteRepoOutput {
    let project = display::clean_path_string(&case.project_root.to_string_lossy());
    let output_dir = display::clean_path_string(&repo_output_dir.to_string_lossy());
    let anchor_statuses = case
        .anchors
        .iter()
        .map(|anchor| DrillSummaryAnchorStatusOutput {
            anchor: anchor.clone(),
            status: "not_run".to_string(),
            typed_hit_count: 0,
            selected: None,
            selected_node_id: None,
            selected_node_ref: None,
            selected_kind: None,
            selected_file_path: None,
            selected_line: None,
            caller_count: 0,
            consumer_count: 0,
            text_hint_count: 0,
            command_count: 0,
            failed_command_count: 0,
            command_duration_ms: 0,
            total_duration_ms: 0,
            resolution_duration_ms: 0,
            consumer_summary_duration_ms: 0,
            slowest_command: None,
            slowest_command_ms: 0,
            source_truth_target_count: 0,
        })
        .collect::<Vec<_>>();
    let next_action = format!(
        "Fix or skip this case, then rerun `drill-suite`; blocked before evidence artifacts were written: {}",
        error.replace('|', "\\|")
    );

    DrillSuiteRepoOutput {
        slug: case.slug.clone(),
        project: project.clone(),
        question: case.question.clone(),
        anchors: case.anchors.clone(),
        output_dir: output_dir.clone(),
        artifact_extension: drill_artifact_extension(format).to_string(),
        summary: DrillSummaryOutput {
            summary_version: 1,
            project,
            label: Some(case.slug.clone()),
            question: Some(case.question.clone()),
            output_dir: output_dir.clone(),
            full_report_json: String::new(),
            full_report_markdown: String::new(),
            mechanical: DrillSummaryMechanicalOutput {
                refresh: refresh_label(refresh, None),
                before: Some(drill_summary_stats(0, 0, 0, 0)),
                before_unavailable_reason: None,
                after: drill_summary_stats(0, 0, 0, 1),
                index_ready: false,
                error_delta: Some(1),
                retrieval_status: None,
                freshness_status: Some("unknown".to_string()),
                stale_file_count: 0,
                freshness_samples: Vec::new(),
                phase_timing_available: false,
                drill_timings: DrillRuntimeTimingsOutput::default(),
            },
            anchors: DrillSummaryAnchorsOutput {
                requested: case.anchors.len(),
                resolved: 0,
                unresolved: case.anchors.len(),
                failed_command_count: 1,
                statuses: anchor_statuses,
            },
            bridges: DrillSummaryBridgesOutput {
                total: 0,
                graph_path: 0,
                partial: 0,
                unresolved_or_error: 0,
                statuses: Vec::new(),
            },
            source_truth: DrillSummarySourceTruthOutput {
                required: false,
                check_count: 0,
                pending_check_count: 0,
                verified_check_count: 0,
                target_file_count: 0,
                target_files: Vec::new(),
                target_file_details: Vec::new(),
                checklist_item_count: 0,
                claim_count: 0,
                pending_claim_count: 0,
                verified_claim_count: 0,
            },
            open_gaps: DrillSummaryOpenGapsOutput {
                overall_status: ClaimReadinessDto::NeedsSourceRead,
                answer_quality_status: "blocked_before_evidence".to_string(),
                safe_to_say_count: 0,
                inferred_claim_count: 0,
                needs_verification_count: 1,
                needs_verification_claim_count: 0,
                pending_claim_count: 0,
                pending_source_truth_check_count: 0,
                next_command_count: 1,
                open_gap_friendly: true,
                status: "blocked".to_string(),
            },
            verdict: DrillSummaryVerdictOutput {
                status: "blocked".to_string(),
                reason: format!("drill failed before evidence collection: {error}"),
                next_action,
            },
        },
        expectations: case.expectations.clone(),
    }
}

pub(super) fn write_drill_suite_outputs(
    format: args::OutputFormat,
    output_dir: &std::path::Path,
    output: &DrillSuiteOutput,
) -> Result<()> {
    let markdown = render_drill_suite_markdown(output);
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(output).context("Failed to serialize drill suite JSON")?,
    );
    write_drill_report_file(&output_dir.join("suite-report.md"), &markdown)?;
    write_drill_report_file(&output_dir.join("suite-report.json"), &json)?;
    let selected = match format {
        args::OutputFormat::Markdown => ensure_trailing_newline(markdown),
        args::OutputFormat::Json => json,
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    };
    let report_ext = drill_artifact_extension(format);
    write_drill_report_file(
        &output_dir.join(format!("drill-suite-report.{report_ext}")),
        &selected,
    )
}

pub(super) fn drill_artifact_extension(format: args::OutputFormat) -> &'static str {
    match format {
        args::OutputFormat::Markdown => "md",
        args::OutputFormat::Json => "json",
        args::OutputFormat::Dot => unreachable!("dot was rejected above"),
    }
}

pub(super) fn render_drill_suite_markdown(output: &DrillSuiteOutput) -> String {
    let mut markdown = String::new();
    render_drill_suite_header(&mut markdown, output);
    render_drill_suite_retrieval_blockers(&mut markdown, &output.retrieval_blockers);
    render_drill_suite_repo_table(&mut markdown, &output.repos);
    render_drill_suite_repo_artifacts(&mut markdown, &output.repos);
    render_drill_suite_next_actions(&mut markdown, &output.next_actions);
    ensure_trailing_newline(markdown)
}

pub(super) fn render_drill_suite_header(markdown: &mut String, output: &DrillSuiteOutput) {
    let _ = writeln!(markdown, "# CodeStory Real-Repo Agent Drill Suite");
    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "- suite: `{}`", output.suite);
    let _ = writeln!(markdown, "- project: `{}`", output.project);
    let _ = writeln!(markdown, "- case_file: `{}`", output.case_file);
    let _ = writeln!(markdown, "- output_dir: `{}`", output.output_dir);
    let _ = writeln!(
        markdown,
        "- repos: {} total, {} ready, {} degraded, {} blocked",
        output.repo_count, output.ready_count, output.degraded_count, output.blocked_count
    );
}

pub(super) fn render_drill_suite_retrieval_blockers(
    markdown: &mut String,
    blockers: &[DrillSuiteRetrievalBlockerOutput],
) {
    if blockers.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Retrieval Blockers");
    for blocker in blockers {
        let _ = writeln!(
            markdown,
            "- `{}` repos={} [{}]: {}",
            blocker.status,
            blocker.repo_count,
            blocker.repos.join(", "),
            blocker.next_action
        );
    }
}

pub(super) fn render_drill_suite_repo_table(markdown: &mut String, repos: &[DrillSuiteRepoOutput]) {
    let _ = writeln!(markdown);
    let _ = writeln!(
        markdown,
        "| repo | verdict | freshness | retrieval | anchors | bridges | source truth | reports | next action |"
    );
    let _ = writeln!(markdown, "|---|---|---|---|---:|---:|---|---|---|");
    for repo in repos {
        let reports = drill_suite_repo_report_label(repo);
        let _ = writeln!(
            markdown,
            "| `{}` | {} | {} | {} | {}/{} | {} | {} | {} | {} |",
            repo.slug,
            repo.summary.verdict.status,
            repo.summary
                .mechanical
                .freshness_status
                .as_deref()
                .unwrap_or("unknown"),
            drill_suite_retrieval_label(repo.summary.mechanical.retrieval_status.as_deref()),
            repo.summary.anchors.resolved,
            repo.summary.anchors.requested,
            drill_suite_bridge_label(&repo.summary.bridges),
            drill_suite_source_truth_label(&repo.summary.source_truth),
            reports,
            repo.summary.verdict.next_action.replace('|', "\\|")
        );
    }
}

pub(super) fn render_drill_suite_repo_artifacts(
    markdown: &mut String,
    repos: &[DrillSuiteRepoOutput],
) {
    if repos.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Repo Artifacts");
    for repo in repos {
        if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty()
        {
            let _ = writeln!(
                markdown,
                "- `{}`: no per-repo artifacts were written because the case blocked before evidence collection",
                repo.slug
            );
            continue;
        }
        let markdown_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
        let json_report =
            drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
        let bridge_artifacts = drill_suite_join_artifact_path(
            &repo.output_dir,
            &format!("*-bridge.{}", repo.artifact_extension),
        );
        let _ = writeln!(
            markdown,
            "- `{}`: report `{}`; json `{}`; bridge artifacts `{}`",
            repo.slug, markdown_report, json_report, bridge_artifacts
        );
    }
}

pub(super) fn render_drill_suite_next_actions(markdown: &mut String, next_actions: &[String]) {
    if next_actions.is_empty() {
        return;
    }

    let _ = writeln!(markdown);
    let _ = writeln!(markdown, "## Next Actions");
    for action in next_actions {
        let _ = writeln!(markdown, "- {action}");
    }
}

pub(super) fn drill_suite_repo_report_label(repo: &DrillSuiteRepoOutput) -> String {
    if repo.summary.full_report_markdown.is_empty() && repo.summary.full_report_json.is_empty() {
        return "not written (blocked before evidence)".to_string();
    }
    let markdown_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_markdown);
    let json_report =
        drill_suite_join_artifact_path(&repo.output_dir, &repo.summary.full_report_json);
    format!("`{markdown_report}` / `{json_report}`").replace('|', "\\|")
}

pub(super) fn drill_suite_join_artifact_path(output_dir: &str, artifact: &str) -> String {
    if artifact.contains(':')
        || artifact.starts_with('/')
        || artifact.starts_with('\\')
        || artifact.contains('/')
        || artifact.contains('\\')
    {
        return artifact.to_string();
    }
    format!(
        "{}/{}",
        output_dir.trim_end_matches(['/', '\\']),
        artifact.trim_start_matches(['/', '\\'])
    )
}

pub(super) fn drill_suite_bridge_label(bridges: &DrillSummaryBridgesOutput) -> String {
    format!(
        "{} graph / {} partial / {} unresolved-error",
        bridges.graph_path, bridges.partial, bridges.unresolved_or_error
    )
}

pub(super) fn drill_suite_source_truth_label(
    source_truth: &DrillSummarySourceTruthOutput,
) -> String {
    if source_truth.required
        || source_truth.pending_check_count > 0
        || source_truth.verified_check_count > 0
    {
        return format!(
            "{} targets / {} verified / {} pending",
            source_truth.target_file_count,
            source_truth.verified_check_count,
            source_truth.pending_check_count
        );
    }
    format!(
        "{} targets / {} checks",
        source_truth.target_file_count, source_truth.check_count
    )
}

pub(super) fn drill_suite_retrieval_blockers(
    repos: &[DrillSuiteRepoOutput],
) -> Vec<DrillSuiteRetrievalBlockerOutput> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for repo in repos {
        let Some(status) = repo.summary.mechanical.retrieval_status.as_ref() else {
            continue;
        };
        if drill_suite_retrieval_label(Some(status)) == "full" {
            continue;
        }
        grouped
            .entry(status.clone())
            .or_default()
            .push(repo.slug.clone());
    }
    grouped
        .into_iter()
        .map(|(status, repos)| {
            let next_action = if status.contains("MissingEmbeddingRuntime") {
                "rebuild with `codestory-cli retrieval index --project <repo> --refresh full`; the embedded engine initializes automatically".to_string()
            } else if status.contains("MissingSemanticDocs") {
                "rerun `codestory-cli retrieval index --project <repo> --refresh full` before trusting packet/search evidence".to_string()
            } else {
                "inspect doctor/retrieval status and repair to retrieval_mode=full before treating broad search quality as repo-specific".to_string()
            };
            DrillSuiteRetrievalBlockerOutput {
                status,
                repo_count: repos.len(),
                repos,
                next_action,
            }
        })
        .collect()
}

pub(super) fn drill_suite_text_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn validate_drill_output_dir(output_dir: &std::path::Path) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create drill output directory {}",
            display::clean_path_string(&output_dir.to_string_lossy())
        )
    })
}

pub(super) struct DrillReportContents {
    selected: String,
    markdown: String,
    json: String,
}

pub(super) fn render_drill_contents(
    format: args::OutputFormat,
    operation: &codestory_runtime::PublicOperation<DrillOutput>,
    markdown: &str,
) -> Result<DrillReportContents> {
    let markdown = ensure_trailing_newline(markdown.to_string());
    let output = runtime::public_operation_json_value(operation, &operation.value)?;
    let json = ensure_trailing_newline(
        serde_json::to_string_pretty(&output).context("Failed to serialize drill JSON")?,
    );
    let selected = match format {
        args::OutputFormat::Markdown => markdown.clone(),
        args::OutputFormat::Json => json.clone(),
        args::OutputFormat::Dot => bail!("--format dot is only supported by `trail`"),
    };
    Ok(DrillReportContents {
        selected,
        markdown,
        json,
    })
}

pub(super) fn write_drill_report_file(path: &std::path::Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| {
        format!(
            "Failed to write drill report {}",
            display::clean_path_string(&path.to_string_lossy())
        )
    })
}

pub(super) fn drill_summary(output: &DrillOutput) -> DrillSummaryOutput {
    let sufficiency = &output.evidence_packet.sufficiency;
    let before_stats = match (
        output.mechanical.before_files,
        output.mechanical.before_nodes,
        output.mechanical.before_edges,
        output.mechanical.before_errors,
    ) {
        (Some(files), Some(nodes), Some(edges), Some(errors)) => {
            Some(drill_summary_stats(files, nodes, edges, errors))
        }
        _ => None,
    };
    let anchor_statuses: Vec<_> = output
        .anchors
        .iter()
        .map(|anchor| {
            let failed_command_count = anchor
                .commands
                .iter()
                .filter(|command| command.status != "ok")
                .count();
            let command_duration_ms = anchor
                .commands
                .iter()
                .map(|command| command.duration_ms)
                .sum();
            let slowest = anchor
                .commands
                .iter()
                .max_by_key(|command| command.duration_ms);
            DrillSummaryAnchorStatusOutput {
                anchor: anchor.anchor.clone(),
                status: if anchor.chosen_anchor.is_some() {
                    "resolved".to_string()
                } else {
                    "unresolved".to_string()
                },
                typed_hit_count: anchor.typed_hit_count,
                selected: anchor
                    .chosen_anchor
                    .as_ref()
                    .map(|hit| hit.display_name.clone()),
                selected_node_id: anchor.chosen_anchor.as_ref().map(|hit| hit.node_id.clone()),
                selected_node_ref: anchor
                    .chosen_anchor
                    .as_ref()
                    .and_then(|hit| hit.node_ref.clone()),
                selected_kind: anchor.chosen_anchor.as_ref().map(|hit| hit.kind),
                selected_file_path: anchor
                    .chosen_anchor
                    .as_ref()
                    .and_then(|hit| hit.file_path.clone()),
                selected_line: anchor.chosen_anchor.as_ref().and_then(|hit| hit.line),
                caller_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.caller_count)
                    .unwrap_or_default(),
                consumer_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.consumer_count)
                    .unwrap_or_default(),
                text_hint_count: anchor
                    .consumer_summary
                    .as_ref()
                    .map(|summary| summary.text_hint_count)
                    .unwrap_or_default(),
                command_count: anchor.commands.len(),
                failed_command_count,
                command_duration_ms,
                total_duration_ms: anchor.timings.total_ms,
                resolution_duration_ms: anchor.timings.resolution_ms,
                consumer_summary_duration_ms: anchor.timings.consumer_summary_ms,
                slowest_command: slowest.map(|command| command.command.clone()),
                slowest_command_ms: slowest
                    .map(|command| command.duration_ms)
                    .unwrap_or_default(),
                source_truth_target_count: anchor.verification_targets.len(),
            }
        })
        .collect();
    let resolved = anchor_statuses
        .iter()
        .filter(|anchor| anchor.status == "resolved")
        .count();
    let failed_anchor_commands = anchor_statuses
        .iter()
        .map(|anchor| anchor.failed_command_count)
        .sum();

    let bridge_statuses: Vec<_> = output
        .bridges
        .iter()
        .map(|bridge| DrillSummaryBridgeStatusOutput {
            from_anchor: bridge.evidence.from_anchor.clone(),
            to_anchor: bridge.evidence.to_anchor.clone(),
            status: bridge.evidence.status.clone(),
            confidence: bridge.evidence.confidence.clone(),
            strategy: bridge.evidence.strategy.clone(),
            command_status: bridge.command.status.clone(),
        })
        .collect();
    let graph_path = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_graph(&bridge.status))
        .count();
    let partial = bridge_statuses
        .iter()
        .filter(|bridge| drill_bridge_status_is_partial(&bridge.status))
        .count();
    let unresolved_or_error = bridge_statuses
        .iter()
        .filter(|bridge| {
            drill_bridge_status_is_unresolved(&bridge.status) || bridge.command_status != "ok"
        })
        .count();

    let mut target_files: Vec<_> = output
        .verification_targets
        .iter()
        .map(|target| target.path.clone())
        .collect();
    dedupe_and_rank_drill_files(&mut target_files);
    let target_file_count = target_files.len();
    let target_file_details =
        drill_summary_source_truth_target_details(&target_files, &output.verification_targets);

    let has_source_truth_checks = !target_files.is_empty();
    let needs_source_truth = sufficiency.status != PacketSufficiencyStatusDto::Sufficient;
    let stale_freshness = output
        .mechanical
        .freshness
        .as_ref()
        .is_some_and(|freshness| freshness.status == IndexFreshnessStatusDto::Stale);
    let open_gap_friendly = !sufficiency.gaps.is_empty()
        || !sufficiency.open_next.is_empty()
        || needs_source_truth
        || stale_freshness;

    DrillSummaryOutput {
        summary_version: 1,
        project: output.project.clone(),
        label: output.label.clone(),
        question: output.question.clone(),
        output_dir: output.output_dir.clone(),
        full_report_json: "drill-report.json".to_string(),
        full_report_markdown: "drill-report.md".to_string(),
        mechanical: DrillSummaryMechanicalOutput {
            refresh: output.mechanical.refresh.clone(),
            before: before_stats,
            before_unavailable_reason: output.mechanical.before_unavailable_reason.clone(),
            after: drill_summary_stats(
                output.mechanical.after_files,
                output.mechanical.after_nodes,
                output.mechanical.after_edges,
                output.mechanical.after_errors,
            ),
            index_ready: output.mechanical.after_files > 0 && output.mechanical.after_errors == 0,
            error_delta: output.mechanical.before_errors.map(|before_errors| {
                i64::from(output.mechanical.after_errors) - i64::from(before_errors)
            }),
            retrieval_status: output
                .mechanical
                .retrieval
                .as_ref()
                .map(|retrieval| {
                    drill_summary_retrieval_status(
                        retrieval,
                        output.mechanical.sidecar_retrieval_mode.as_deref(),
                    )
                })
                .or_else(|| output.mechanical.sidecar_retrieval_mode.clone()),
            freshness_status: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_freshness_status),
            stale_file_count: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_stale_file_count)
                .unwrap_or_default(),
            freshness_samples: output
                .mechanical
                .freshness
                .as_ref()
                .map(drill_summary_freshness_samples)
                .unwrap_or_default(),
            phase_timing_available: output.mechanical.phase_timings.is_some(),
            drill_timings: output.mechanical.drill_timings.clone(),
        },
        anchors: DrillSummaryAnchorsOutput {
            requested: output.anchors.len(),
            resolved,
            unresolved: output.anchors.len().saturating_sub(resolved),
            failed_command_count: failed_anchor_commands,
            statuses: anchor_statuses,
        },
        bridges: DrillSummaryBridgesOutput {
            total: output.bridges.len(),
            graph_path,
            partial,
            unresolved_or_error,
            statuses: bridge_statuses,
        },
        source_truth: DrillSummarySourceTruthOutput {
            required: needs_source_truth,
            check_count: target_file_count,
            pending_check_count: if has_source_truth_checks {
                usize::from(needs_source_truth) * target_file_count
            } else {
                0
            },
            verified_check_count: if needs_source_truth {
                0
            } else {
                target_file_count
            },
            target_file_count,
            target_files,
            target_file_details,
            checklist_item_count: 0,
            claim_count: sufficiency.covered_claims.len(),
            pending_claim_count: sufficiency.gaps.len(),
            verified_claim_count: sufficiency.covered_claims.len(),
        },
        open_gaps: DrillSummaryOpenGapsOutput {
            overall_status: drill_packet_claim_readiness(sufficiency.status),
            answer_quality_status: packet_sufficiency_label(sufficiency.status).to_string(),
            safe_to_say_count: sufficiency.covered_claims.len(),
            inferred_claim_count: sufficiency
                .covered_claims
                .iter()
                .filter(|claim| claim.proof_status != Some(PacketProofStatusDto::Proven))
                .count(),
            needs_verification_count: sufficiency.gaps.len(),
            needs_verification_claim_count: sufficiency.gaps.len(),
            pending_claim_count: if needs_source_truth {
                sufficiency.gaps.len()
            } else {
                0
            },
            pending_source_truth_check_count: if needs_source_truth {
                target_file_count
            } else {
                0
            },
            next_command_count: sufficiency.follow_up_commands.len(),
            open_gap_friendly,
            status: if open_gap_friendly {
                "open_gaps_explicit".to_string()
            } else {
                "no_open_gaps_reported".to_string()
            },
        },
        verdict: drill_summary_verdict(
            output,
            resolved,
            graph_path,
            partial,
            unresolved_or_error,
            needs_source_truth,
            open_gap_friendly,
            stale_freshness,
        ),
    }
}

pub(super) fn drill_packet_claim_readiness(
    status: PacketSufficiencyStatusDto,
) -> ClaimReadinessDto {
    match status {
        PacketSufficiencyStatusDto::Sufficient => ClaimReadinessDto::Supported,
        PacketSufficiencyStatusDto::Partial => ClaimReadinessDto::Partial,
        PacketSufficiencyStatusDto::Insufficient => ClaimReadinessDto::NeedsSourceRead,
    }
}

pub(super) fn drill_bridge_status_is_graph(status: &str) -> bool {
    matches!(
        status,
        "graph_path" | "reverse_graph_path" | "graph_shared_file"
    )
}

pub(super) fn drill_bridge_status_is_partial(status: &str) -> bool {
    matches!(
        status,
        "shared_file_only"
            | "evidence_hint_only"
            | "framework_route"
            | "component_usage"
            | "data_collection_usage"
            | "source_truth_only"
    )
}

pub(super) fn drill_bridge_status_is_unresolved(status: &str) -> bool {
    matches!(status, "no_bridge_found" | "unresolved_anchor" | "error")
}

#[allow(clippy::too_many_arguments)]
pub(super) fn drill_summary_verdict(
    output: &DrillOutput,
    resolved_anchors: usize,
    graph_path_bridges: usize,
    partial_bridges: usize,
    unresolved_or_error_bridges: usize,
    needs_source_truth: bool,
    open_gap_friendly: bool,
    stale_freshness: bool,
) -> DrillSummaryVerdictOutput {
    let failed_anchor_commands = output
        .anchors
        .iter()
        .flat_map(|anchor| anchor.commands.iter())
        .filter(|command| command.status != "ok")
        .count();
    let unresolved_anchors = output.anchors.len().saturating_sub(resolved_anchors);
    if output.mechanical.after_files == 0 || output.mechanical.after_errors > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: "index is not ready or contains indexing errors".to_string(),
            next_action: "inspect doctor/index output before trusting drill evidence".to_string(),
        };
    }
    if unresolved_anchors > 0 || failed_anchor_commands > 0 {
        return DrillSummaryVerdictOutput {
            status: "blocked".to_string(),
            reason: format!(
                "unresolved_anchors={unresolved_anchors} failed_anchor_commands={failed_anchor_commands}"
            ),
            next_action: "repair anchor selection or inspect command errors before answering"
                .to_string(),
        };
    }
    if stale_freshness {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "index_freshness=stale source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.verification_targets.len()
            ),
            next_action: drill_stale_freshness_next_action(output),
        };
    }
    if needs_source_truth || open_gap_friendly || unresolved_or_error_bridges > 0 {
        return DrillSummaryVerdictOutput {
            status: "degraded".to_string(),
            reason: format!(
                "source_truth_required={} graph_bridges={graph_path_bridges}/{} partial_bridges={partial_bridges} unresolved_or_error_bridges={unresolved_or_error_bridges} pending_source_truth_checks={}",
                needs_source_truth,
                output.bridges.len(),
                output.verification_targets.len()
            ),
            next_action: drill_degraded_next_action(output, unresolved_or_error_bridges),
        };
    }
    DrillSummaryVerdictOutput {
        status: "ready".to_string(),
        reason: "all anchors resolved and no open source-truth blockers were reported".to_string(),
        next_action: "answer from the evidence packet and keep source verification focused"
            .to_string(),
    }
}

pub(super) fn drill_stale_freshness_next_action(output: &DrillOutput) -> String {
    let project = quote_command_path(std::path::Path::new(&output.project));
    let mut action = format!(
        "refresh stale index evidence first with `codestory-cli index --project {project} --refresh incremental`, then rerun drill before finalizing"
    );
    if let Some(freshness) = output.mechanical.freshness.as_ref() {
        let samples = freshness
            .samples
            .iter()
            .take(3)
            .map(|sample| sample.path.clone())
            .collect::<Vec<_>>();
        if !samples.is_empty() {
            let _ = write!(action, "; stale samples: {}", samples.join("; "));
        }
    }
    action
}

pub(super) fn drill_degraded_next_action(
    output: &DrillOutput,
    unresolved_or_error_bridges: usize,
) -> String {
    let failed_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| bridge.command.status != "ok" || bridge.evidence.status == "error")
        .count();
    if failed_bridge_count > 0 {
        return format!(
            "repair or rerun {failed_bridge_count} failed bridge evidence command(s) before treating degraded bridges as verification targets"
        );
    }
    let degraded_bridge_count = output
        .bridges
        .iter()
        .filter(|bridge| !drill_bridge_status_is_graph(&bridge.evidence.status))
        .count()
        .max(unresolved_or_error_bridges);
    let mut files = output
        .verification_targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<Vec<_>>();
    dedupe_and_rank_drill_files(&mut files);

    let mut action = "write a CodeStory-only draft".to_string();
    let pending_claim_count = output.evidence_packet.sufficiency.gaps.len();
    if pending_claim_count > 0 && degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s), starting with {degraded_bridge_count} degraded bridge(s)"
        );
    } else if pending_claim_count > 0 {
        let _ = write!(
            action,
            ", then verify {pending_claim_count} pending claim(s)"
        );
    } else if degraded_bridge_count > 0 {
        let _ = write!(
            action,
            ", then verify {degraded_bridge_count} degraded bridge(s)"
        );
    } else {
        action.push_str(", then verify source-truth targets");
    }
    if !files.is_empty() {
        let preview = files.into_iter().take(3).collect::<Vec<_>>().join("; ");
        let _ = write!(action, " including {preview}");
    }
    if !output
        .evidence_packet
        .sufficiency
        .follow_up_commands
        .is_empty()
    {
        action.push_str("; use emitted packet follow-up commands before finalizing");
    }
    action
}

pub(super) fn drill_summary_stats(
    files: u32,
    nodes: u32,
    edges: u32,
    errors: u32,
) -> DrillSummaryStatsOutput {
    DrillSummaryStatsOutput {
        files,
        nodes,
        edges,
        errors,
    }
}

pub(super) fn drill_summary_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
    sidecar_retrieval_mode: Option<&str>,
) -> String {
    if let Some(mode) = sidecar_retrieval_mode {
        if mode == "full" {
            return "full".to_string();
        }
        return format!(
            "{mode}:retrieval_degraded; legacy={}",
            drill_summary_legacy_retrieval_status(retrieval)
        );
    }
    drill_summary_legacy_retrieval_status(retrieval)
}

pub(super) fn drill_summary_legacy_retrieval_status(
    retrieval: &codestory_contracts::api::RetrievalStateDto,
) -> String {
    let mode = match retrieval.mode {
        codestory_contracts::api::RetrievalModeDto::Hybrid => "hybrid",
        codestory_contracts::api::RetrievalModeDto::Symbolic => "symbolic",
    };
    let readiness = if retrieval.semantic_ready {
        "semantic_ready"
    } else {
        "semantic_unavailable"
    };
    match retrieval.fallback_reason {
        Some(reason) => format!("{mode}:{readiness}:diagnostic={reason:?}"),
        None => format!("{mode}:{readiness}"),
    }
}

pub(super) fn drill_suite_retrieval_label(status: Option<&str>) -> &str {
    match status {
        Some("full") => "full",
        Some(value) if value.contains("retrieval_degraded") => "needs-retrieval-refresh",
        Some(value) if value.contains("semantic_ready") || value == "hybrid-ready" => "degraded",
        Some(value) if value.contains("semantic_unavailable") => "needs-retrieval-refresh",
        Some("hybrid") => "degraded",
        Some("symbolic") => "needs-retrieval-refresh",
        Some(_) => "partial",
        None => "unknown",
    }
}

pub(super) fn drill_summary_source_truth_target_details(
    target_files: &[String],
    targets: &[VerificationTargetOutput],
) -> Vec<DrillSummarySourceTruthTargetOutput> {
    target_files
        .iter()
        .map(|path| {
            let check_reasons = targets
                .iter()
                .filter(|target| normalize_drill_path(&target.path) == normalize_drill_path(path))
                .map(|target| target.reason.clone())
                .collect::<Vec<_>>();
            let role = drill_source_truth_target_role(path, &check_reasons);
            DrillSummarySourceTruthTargetOutput {
                path: path.clone(),
                role: role.clone(),
                rank_reason: drill_source_truth_target_rank_reason(path, &role),
                check_reasons,
            }
        })
        .collect()
}

pub(super) fn normalize_drill_path(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

pub(super) fn drill_path_is_framework_route_or_page(path: &str) -> bool {
    let normalized = normalize_drill_path(path);
    normalized.ends_with("/route.ts")
        || normalized.ends_with("/route.tsx")
        || normalized.ends_with("/route.js")
        || normalized.ends_with("/route.jsx")
        || normalized.ends_with("/page.tsx")
        || normalized.ends_with("/page.jsx")
        || ((normalized.contains("/app/") || normalized.contains("/pages/"))
            && (normalized.ends_with(".tsx") || normalized.ends_with(".jsx")))
}

pub(super) fn drill_source_truth_target_role(path: &str, reasons: &[String]) -> String {
    let path = normalize_drill_path(path);
    let reason_text = reasons.join(" ").to_ascii_lowercase();
    if drill_path_is_framework_route_or_page(&path) {
        return "public_surface".to_string();
    }
    if path.contains("/components/") && !path.contains("/components/admin") {
        return "runtime_entrypoint".to_string();
    }
    if path.contains("/collections/") || reason_text.contains("collection") {
        return "data_store".to_string();
    }
    if path.contains("comment-auth") || reason_text.contains("auth") {
        return "comment_auth".to_string();
    }
    if path.contains("/tests/") || path.contains(".spec.") || path.contains(".test.") {
        return "test_support".to_string();
    }
    if path.contains("/admin/") || path.contains("/components/admin") {
        return "admin_support".to_string();
    }
    if drill_bridge_evidence_is_generated_path(&format!("/{path}")) {
        return "generated_or_auxiliary".to_string();
    }
    "anchor_definition".to_string()
}

pub(super) fn drill_source_truth_target_rank_reason(path: &str, role: &str) -> String {
    match role {
        "public_surface" => "ranked ahead as public runtime surface evidence".to_string(),
        "runtime_entrypoint" => "ranked ahead as runtime/component evidence".to_string(),
        "data_store" => "kept as Payload/data-store evidence".to_string(),
        "comment_auth" => "kept as comment authentication evidence".to_string(),
        "test_support" => "demoted behind runtime evidence as test support".to_string(),
        "admin_support" => "demoted behind public runtime evidence as admin support".to_string(),
        "generated_or_auxiliary" => {
            "demoted behind source files as generated or auxiliary evidence".to_string()
        }
        _ if normalize_drill_path(path).contains("/src/") => {
            "ranked as production source evidence".to_string()
        }
        _ => "ranked after primary source surfaces".to_string(),
    }
}

pub(super) fn drill_summary_freshness_status(freshness: &IndexFreshnessDto) -> String {
    match freshness.status {
        IndexFreshnessStatusDto::Fresh => "fresh".to_string(),
        IndexFreshnessStatusDto::Stale => "stale".to_string(),
        IndexFreshnessStatusDto::NotChecked => "not_checked".to_string(),
    }
}

pub(super) fn drill_summary_stale_file_count(freshness: &IndexFreshnessDto) -> u32 {
    if freshness.status == IndexFreshnessStatusDto::Stale {
        freshness
            .changed_file_count
            .saturating_add(freshness.new_file_count)
            .saturating_add(freshness.removed_file_count)
    } else {
        0
    }
}

pub(super) fn drill_summary_freshness_samples(freshness: &IndexFreshnessDto) -> Vec<String> {
    freshness
        .samples
        .iter()
        .take(8)
        .map(|sample| format!("{:?}: {}", sample.kind, sample.path))
        .collect()
}

pub(super) fn ensure_trailing_newline(mut content: String) -> String {
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content
}

pub(super) fn output_slug(value: &str) -> String {
    let slug = value.chars().fold(String::new(), |mut slug, ch| {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        slug
    });
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "anchor".to_string()
    } else {
        slug.to_string()
    }
}

pub(super) fn dedupe_verification_targets(targets: &mut Vec<VerificationTargetOutput>) {
    let mut seen = HashSet::new();
    targets.retain(|target| {
        seen.insert((
            target.role.clone(),
            target.path.clone(),
            target.line,
            target.reason.clone(),
        ))
    });
}

pub(super) fn dedupe_and_rank_drill_files(files: &mut Vec<String>) {
    files.sort_by_cached_key(|path| normalize_drill_path(path));
    files.dedup_by(|left, right| normalize_drill_path(left) == normalize_drill_path(right));
}

pub(super) fn drill_bridge_evidence_is_generated_path(normalized_with_root: &str) -> bool {
    normalized_with_root.contains("/target/")
        || normalized_with_root.contains("/dist/")
        || normalized_with_root.contains("/build/")
        || normalized_with_root.contains("/node_modules/")
}

pub(super) fn search_output_from_results(
    runtime: &RuntimeContext,
    search_results: &codestory_contracts::api::SearchResultsDto,
    include_score_details: bool,
) -> SearchOutput {
    let occurrences = collect_search_hit_occurrences(
        runtime,
        search_results
            .indexed_symbol_hits
            .iter()
            .chain(search_results.suggestions.iter()),
    );
    build_search_output(SearchOutputParts {
        project_root: &runtime.project_root,
        query: &search_results.query,
        retrieval: &search_results.retrieval,
        retrieval_shadow: search_results.retrieval_shadow.as_ref(),
        freshness: search_results.freshness.as_ref(),
        symbol_hits: &search_results.indexed_symbol_hits,
        repo_text_hits: &search_results.repo_text_hits,
        repo_text_stats: search_results.repo_text_stats.as_ref(),
        query_assessment: search_results.query_assessment.as_ref(),
        search_plan: search_results.search_plan.as_ref(),
        suggestions: &search_results.suggestions,
        occurrences_by_node: &occurrences,
        limit_per_source: search_results.limit_per_source,
        repo_text: RepoTextOutputConfig {
            mode: from_api_repo_text_mode(search_results.repo_text_mode),
            enabled: search_results.repo_text_enabled,
        },
        explain: include_score_details,
    })
}
