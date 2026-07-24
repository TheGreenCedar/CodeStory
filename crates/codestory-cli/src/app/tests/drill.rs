use super::test_support::*;
use super::*;

#[test]
fn drill_packet_adapter_reuses_packet_citations_and_sufficiency() {
    let packet = sample_task_brief_packet();
    let citations = drill_packet_citations(&packet);
    let anchor_name = packet.answer.citations[0].display_name.clone();
    let anchors = drill_packet_anchors(
        Path::new("C:/repo"),
        std::slice::from_ref(&anchor_name),
        &citations,
    );
    let bridges = drill_packet_bridges(Path::new("C:/repo"), &packet);

    assert_eq!(anchors.len(), 1);
    assert_eq!(
        anchors[0]
            .chosen_anchor
            .as_ref()
            .map(|hit| hit.display_name.as_str()),
        Some(anchor_name.as_str())
    );
    assert_eq!(anchors[0].verification_targets.len(), 1);
    assert_eq!(bridges.len(), 1);
    assert_eq!(bridges[0].evidence.strategy, "packet_claim");
    assert_eq!(bridges[0].evidence.status, "source_truth_only");
    assert_eq!(
        drill_packet_claim_readiness(packet.sufficiency.status),
        ClaimReadinessDto::Partial
    );
    assert_eq!(
        bridges[0].evidence.next_commands,
        packet.sufficiency.follow_up_commands
    );
}

#[test]
fn drill_executes_one_packet_with_explicit_anchor_probes() {
    let packet = sample_task_brief_packet();
    let calls = std::cell::Cell::new(0);
    let request = AgentPacketRequestDto {
        question: packet.question.clone(),
        budget: PacketBudgetModeDto::Standard,
        task_class: None,
        probes: Vec::new(),
        extra_probes: vec!["WorkspaceIndexer".to_string()],
        include_evidence: true,
        latency_budget_ms: None,
    };

    let result = execute_drill_packet(request, |request| {
        calls.set(calls.get() + 1);
        assert_eq!(request.extra_probes, ["WorkspaceIndexer"]);
        Ok(packet.clone())
    })
    .expect("execute packet");

    assert_eq!(calls.get(), 1);
    assert_eq!(result.packet_id, packet.packet_id);
}

#[test]
fn drill_retained_fields_match_pre_adapter_fixture() {
    let mut packet = sample_task_brief_packet();
    let source =
        sample_task_brief_citation("WorkspaceIndexer", NodeKind::FUNCTION, "src/indexer.rs", 12);
    let search = sample_task_brief_citation("SearchService", NodeKind::STRUCT, "src/search.rs", 24);
    packet.question = "How does indexing feed search?".to_string();
    packet.answer.prompt = packet.question.clone();
    packet.answer.citations = vec![source.clone(), search.clone()];
    packet.plan.queries = vec![PacketPlanQueryDto {
        query: "WorkspaceIndexer".to_string(),
        purpose: "explicit symbol probe from packet request".to_string(),
    }];
    packet.sufficiency.covered_claims[0].citations = vec![source, search];
    packet.sufficiency.follow_up_commands =
        vec!["codestory-cli snippet --query WorkspaceIndexer --project .".to_string()];

    let citations = drill_packet_citations(&packet);
    let anchors = drill_packet_anchors(
        Path::new("C:/repo"),
        &["WorkspaceIndexer".to_string()],
        &citations,
    );
    let bridges = drill_packet_bridges(Path::new("C:/repo"), &packet);
    let verification_targets = drill_packet_verification_targets(Path::new("C:/repo"), &citations);
    let output = DrillOutput {
        project: "C:/repo".to_string(),
        label: Some("fixture".to_string()),
        question: Some(packet.question.clone()),
        output_dir: "artifacts/drill".to_string(),
        mechanical: DrillMechanicalOutput {
            before_files: Some(2),
            before_nodes: Some(4),
            before_edges: Some(2),
            before_errors: Some(0),
            before_unavailable_reason: None,
            after_files: 2,
            after_nodes: 4,
            after_edges: 2,
            after_errors: 0,
            refresh: "none".to_string(),
            retrieval: Some(sample_retrieval()),
            sidecar_retrieval_mode: Some("full".to_string()),
            freshness: None,
            phase_timings: None,
            drill_timings: DrillRuntimeTimingsOutput::default(),
        },
        question_search: Some(DrillCommandStatusOutput {
            command: "packet".to_string(),
            status: "partial".to_string(),
            duration_ms: 1,
            artifact: None,
            error: None,
        }),
        question_supplemental_searches: Vec::new(),
        anchors,
        bridges,
        execution_boundaries: vec![DrillExecutionBoundaryOutput {
            command: "packet".to_string(),
            flow: vec!["execute one bounded batch retrieval".to_string()],
            source_files: vec!["crates/codestory-runtime/src/agent/orchestrator.rs".to_string()],
        }],
        verification_targets,
        next_commands: packet.sufficiency.follow_up_commands.clone(),
        evidence_packet: packet,
    };
    let output_dir = tempdir().expect("output dir");
    let mut operation = codestory_runtime::PublicOperation {
        value: output,
        core_publication: None,
        retrieval_publication: None,
        operation_id: "test-drill".to_string(),
        attempt: 1,
    };
    write_drill_outputs(args::OutputFormat::Json, output_dir.path(), &operation)
        .expect("write drill fixtures");

    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(output_dir.path().join("drill-report.json")).expect("read report"),
    )
    .expect("parse report");
    let summary: serde_json::Value = serde_json::from_slice(
        &fs::read(output_dir.path().join("drill-summary.json")).expect("read summary"),
    )
    .expect("parse summary");
    let markdown =
        fs::read_to_string(output_dir.path().join("drill-report.md")).expect("read markdown");
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/drill_packet_parity/retained-fields.json"
    ))
    .expect("parse retained-field fixture");

    for (document, expected) in [
        (&report, &fixture["report"]),
        (&summary, &fixture["summary"]),
    ] {
        for (pointer, expected) in expected.as_object().expect("pointer map") {
            assert_eq!(
                document.pointer(pointer),
                Some(expected),
                "retained field changed at {pointer}"
            );
        }
    }
    for marker in fixture["markdown_contains"]
        .as_array()
        .expect("markdown markers")
    {
        let marker = marker.as_str().expect("markdown marker string");
        assert!(
            markdown.contains(marker),
            "missing retained Markdown `{marker}`"
        );
    }
    let mut artifacts = fs::read_dir(output_dir.path())
        .expect("list artifacts")
        .map(|entry| {
            entry
                .expect("artifact entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    artifacts.sort();
    assert_eq!(
        artifacts,
        fixture["artifacts"]
            .as_array()
            .expect("artifact names")
            .iter()
            .map(|value| value.as_str().expect("artifact name").to_string())
            .collect::<Vec<_>>()
    );

    operation.value.mechanical.before_files = None;
    operation.value.mechanical.before_nodes = None;
    operation.value.mechanical.before_edges = None;
    operation.value.mechanical.before_errors = None;
    operation.value.mechanical.before_unavailable_reason =
        Some("core_schema_upgrade_required".to_string());
    operation.value.mechanical.refresh = "auto->full".to_string();
    let unavailable_dir = tempdir().expect("unavailable output dir");
    write_drill_outputs(args::OutputFormat::Json, unavailable_dir.path(), &operation)
        .expect("write unavailable-before drill fixtures");

    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(unavailable_dir.path().join("drill-report.json"))
            .expect("read unavailable-before report"),
    )
    .expect("parse unavailable-before report");
    let summary: serde_json::Value = serde_json::from_slice(
        &fs::read(unavailable_dir.path().join("drill-summary.json"))
            .expect("read unavailable-before summary"),
    )
    .expect("parse unavailable-before summary");
    let markdown = fs::read_to_string(unavailable_dir.path().join("drill-report.md"))
        .expect("read unavailable-before markdown");

    for field in [
        "/mechanical/before_files",
        "/mechanical/before_nodes",
        "/mechanical/before_edges",
        "/mechanical/before_errors",
    ] {
        assert!(report.pointer(field).is_none(), "unexpected {field}");
    }
    assert_eq!(
        report.pointer("/mechanical/before_unavailable_reason"),
        Some(&serde_json::json!("core_schema_upgrade_required"))
    );
    assert!(summary.pointer("/mechanical/before").is_none());
    assert!(summary.pointer("/mechanical/error_delta").is_none());
    assert_eq!(
        summary.pointer("/mechanical/before_unavailable_reason"),
        Some(&serde_json::json!("core_schema_upgrade_required"))
    );
    assert!(
        markdown.contains("index_before: unavailable reason=core_schema_upgrade_required"),
        "{markdown}"
    );
    assert!(!markdown.contains("index_before: files="), "{markdown}");
}

#[test]
fn drill_packet_anchor_rejects_exact_unresolvable_or_unknown_citations() {
    let mut citation =
        sample_task_brief_citation("WorkspaceIndexer", NodeKind::FUNCTION, "src/indexer.rs", 12);
    citation.resolvable = false;
    let anchors = drill_packet_anchors(
        Path::new("C:/repo"),
        &["WorkspaceIndexer".to_string()],
        std::slice::from_ref(&citation),
    );
    assert_eq!(anchors[0].typed_hit_count, 0);
    assert!(anchors[0].chosen_anchor.is_none());
    assert!(anchors[0].verification_targets.is_empty());

    citation.resolvable = true;
    citation.kind = NodeKind::UNKNOWN;
    let anchors = drill_packet_anchors(
        Path::new("C:/repo"),
        &["WorkspaceIndexer".to_string()],
        &[citation],
    );
    assert!(anchors[0].chosen_anchor.is_none());
}

#[test]
fn drill_packet_keeps_structural_source_ranges_navigable_but_not_typed() {
    let project_root = Path::new("C:/repo");
    let mut structural =
        sample_task_brief_citation("Cargo package", NodeKind::PACKAGE, "Cargo.toml", 2);
    structural.evidence_tier =
        Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText);
    structural.evidence_producer = Some("structural_cargo_manifest_collector".to_string());
    structural.resolution_status =
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly);

    let navigable =
        drill_search_hit_from_packet_citation(project_root, "Cargo package", &structural);
    assert!(navigable.resolvable);
    assert!(!drill_packet_citation_is_typed_resolvable(&structural));

    let anchors = drill_packet_anchors(
        project_root,
        &["Cargo package".to_string()],
        std::slice::from_ref(&structural),
    );
    assert_eq!(anchors[0].typed_hit_count, 0);
    assert!(anchors[0].chosen_anchor.is_none());
    assert!(anchors[0].verification_targets.is_empty());
    assert!(drill_packet_verification_targets(project_root, &[structural.clone()]).is_empty());

    let mut packet = sample_task_brief_packet();
    packet.sufficiency.covered_claims[0].citations = vec![
        structural,
        sample_task_brief_citation("SearchService", NodeKind::STRUCT, "src/search.rs", 24),
    ];
    assert!(drill_packet_bridges(project_root, &packet).is_empty());

    let mut source_range_only =
        sample_task_brief_citation("source range", NodeKind::FUNCTION, "src/lib.rs", 8);
    source_range_only.resolution_status =
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly);
    assert!(!drill_packet_citation_is_typed_resolvable(
        &source_range_only
    ));
}

#[test]
fn drill_packet_bridge_requires_shared_concrete_edge_evidence() {
    let mut packet = sample_task_brief_packet();
    packet.sufficiency.covered_claims[0].citations[0].subgraph_id = Some("only-from".to_string());
    let bridges = drill_packet_bridges(Path::new("C:/repo"), &packet);
    assert_eq!(bridges[0].evidence.status, "source_truth_only");

    packet.sufficiency.covered_claims[0].citations[0].subgraph_id = Some("shared".to_string());
    packet.sufficiency.covered_claims[0].citations[1].subgraph_id = Some("shared".to_string());
    let bridges = drill_packet_bridges(Path::new("C:/repo"), &packet);
    assert_eq!(bridges[0].evidence.status, "source_truth_only");

    packet.sufficiency.covered_claims[0].citations[0].subgraph_id = None;
    packet.sufficiency.covered_claims[0].citations[1].subgraph_id = None;
    packet.sufficiency.covered_claims[0].citations[0].evidence_edge_ids =
        vec![EdgeId("shared-edge".to_string())];
    packet.sufficiency.covered_claims[0].citations[1].evidence_edge_ids =
        vec![EdgeId("shared-edge".to_string())];
    let bridges = drill_packet_bridges(Path::new("C:/repo"), &packet);
    assert_eq!(bridges[0].evidence.status, "graph_path");
}
