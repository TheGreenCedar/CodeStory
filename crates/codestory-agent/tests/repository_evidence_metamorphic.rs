//! Visible metamorphic / isomorphic suite for Stage B repository evidence planning.
//!
//! These fixtures prove relationship planning is stable under rename, paraphrase,
//! wrapper depth, multiple implementations, disconnected distractors, truncated
//! graphs, and several language path spellings. Failures must be fixed by
//! redesigning the relationship-planning seam — not by adding noun/classifier
//! rules. Historical 18-task corpora are not a gate here.

use codestory_agent::packet_plan::build_packet_plan_with_extra;
use codestory_agent::repository_evidence_plan::{
    DEFAULT_REPOSITORY_EVIDENCE_LIMITS, RepositoryEvidenceGapKind, RepositoryEvidenceInput,
    RepositoryEvidenceLimits, RepositoryEvidenceObjectiveKind, build_repository_evidence_plan,
};
use codestory_contracts::api::{
    AgentCitationDto, EdgeId, EdgeKind, GraphEdgeDto, NodeId, NodeKind, PacketBudgetModeDto,
    PacketTaskClassDto, SearchHitOrigin,
};

fn citation(id: &str, display: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
    AgentCitationDto {
        node_id: NodeId(id.into()),
        display_name: display.into(),
        kind,
        file_path: Some(path.into()),
        line: Some(1),
        score: 1.0,
        origin: SearchHitOrigin::IndexedSymbol,
        target: None,
        resolvable: true,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        source_excerpt: None,
    }
}

fn call_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId(id.into()),
        source: NodeId(source.into()),
        target: NodeId(target.into()),
        kind: EdgeKind::CALL,
        confidence: None,
        certainty: Some("certain".into()),
        callsite_identity: None,
        candidate_targets: Vec::new(),
    }
}

fn member_edge(id: &str, source: &str, target: &str) -> GraphEdgeDto {
    GraphEdgeDto {
        id: EdgeId(id.into()),
        source: NodeId(source.into()),
        target: NodeId(target.into()),
        kind: EdgeKind::MEMBER,
        confidence: None,
        certainty: Some("certain".into()),
        callsite_identity: None,
        candidate_targets: Vec::new(),
    }
}

fn objective_signature(
    plan: &codestory_agent::repository_evidence_plan::RepositoryEvidencePlan,
) -> Vec<(RepositoryEvidenceObjectiveKind, usize, usize)> {
    plan.objectives
        .iter()
        .map(|o| (o.kind, o.node_ids.len(), o.edge_ids.len()))
        .collect()
}

#[test]
fn rename_bijection_preserves_objective_shape() {
    let left_seeds = [
        citation("a", "Alpha::run", "src/alpha.rs", NodeKind::METHOD),
        citation("b", "Beta::finish", "src/beta.rs", NodeKind::METHOD),
    ];
    let right_seeds = [
        citation("a", "Omega::run", "pkg/omega.py", NodeKind::METHOD),
        citation("b", "Sigma::finish", "pkg/sigma.py", NodeKind::METHOD),
    ];
    let left_edges = [call_edge("e1", "a", "b")];
    let right_edges = [call_edge("e1", "a", "b")];
    let left = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Trace Alpha::run calling Beta::finish",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &left_seeds,
            relations: &left_edges,
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    let right = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Trace Omega::run calling Sigma::finish",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &right_seeds,
            relations: &right_edges,
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    assert_eq!(objective_signature(&left), objective_signature(&right));
    assert_eq!(left.material_edge_ids.len(), right.material_edge_ids.len());
    assert!(
        left.objectives
            .iter()
            .any(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath)
    );
}

#[test]
fn paraphrase_does_not_change_repository_objectives() {
    let seeds = [
        citation("entry", "Entry.handle", "src/entry.go", NodeKind::METHOD),
        citation("sink", "Sink.write", "src/sink.go", NodeKind::METHOD),
    ];
    let edges = [call_edge("c1", "entry", "sink")];
    let paraphrases = [
        "How does Entry.handle reach Sink.write?",
        "Trace the call from Entry.handle to Sink.write.",
        "Explain Entry.handle invoking Sink.write in this repository.",
    ];
    let signatures: Vec<_> = paraphrases
        .iter()
        .map(|question| {
            objective_signature(&build_repository_evidence_plan(
                RepositoryEvidenceInput {
                    question,
                    task_class: PacketTaskClassDto::RouteTracing,
                    seeds: &seeds,
                    relations: &edges,
                },
                DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
            ))
        })
        .collect();
    assert!(signatures.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn wrapper_depth_keeps_shortest_retained_path() {
    let seeds = [
        citation("outer", "Outer.run", "src/outer.ts", NodeKind::FUNCTION),
        citation("inner", "Inner.run", "src/inner.ts", NodeKind::FUNCTION),
    ];
    let edges = [
        call_edge("w1", "outer", "wrap1"),
        call_edge("w2", "wrap1", "wrap2"),
        call_edge("w3", "wrap2", "inner"),
        call_edge("direct", "outer", "inner"),
    ];
    let plan = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Trace Outer.run to Inner.run",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &seeds,
            relations: &edges,
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    let path = plan
        .objectives
        .iter()
        .find(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath)
        .expect("relation path");
    assert_eq!(path.edge_ids, vec![EdgeId("direct".into())]);
}

#[test]
fn multi_impl_membership_selects_implementation_relations() {
    let seeds = [citation(
        "iface",
        "Store",
        "src/store.kt",
        NodeKind::INTERFACE,
    )];
    let edges = [
        member_edge("m1", "iface", "mem_sqlite"),
        member_edge("m2", "iface", "mem_memory"),
        call_edge("noise", "unrelated_a", "unrelated_b"),
    ];
    let plan = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Who implements Store?",
            task_class: PacketTaskClassDto::SymbolOwnership,
            seeds: &seeds,
            relations: &edges,
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    let impls = plan
        .objectives
        .iter()
        .filter(|o| o.kind == RepositoryEvidenceObjectiveKind::ImplementationRelation)
        .count();
    assert_eq!(impls, 2);
    assert!(!plan.material_edge_ids.contains(&EdgeId("noise".into())));
}

#[test]
fn disconnected_distractors_do_not_enter_material_set() {
    let seeds = [
        citation("a", "A.f", "src/a.java", NodeKind::METHOD),
        citation("b", "B.g", "src/b.java", NodeKind::METHOD),
    ];
    let edges = [
        call_edge("keep", "a", "b"),
        call_edge("distract", "x", "y"),
        call_edge("distract2", "y", "z"),
    ];
    let plan = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Trace A.f to B.g",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &seeds,
            relations: &edges,
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    assert!(plan.material_edge_ids.contains(&EdgeId("keep".into())));
    assert!(!plan.material_node_ids.contains(&NodeId("x".into())));
    assert!(!plan.material_edge_ids.contains(&EdgeId("distract".into())));
}

#[test]
fn truncated_and_partial_graphs_report_gaps_not_absence() {
    let seeds = [
        citation("a", "A", "a.rs", NodeKind::FUNCTION),
        citation("b", "B", "b.rs", NodeKind::FUNCTION),
        citation("c", "C", "c.rs", NodeKind::FUNCTION),
    ];
    // No edges between seeds → missing relation / unknown, never inferred absence.
    let plan = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Relate A, B, and C",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &seeds,
            relations: &[],
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    assert!(plan.uncovered.iter().any(|g| matches!(
        g.kind,
        RepositoryEvidenceGapKind::MissingRelation | RepositoryEvidenceGapKind::Unknown
    )));

    let tight = RepositoryEvidenceLimits {
        max_seed_nodes: 2,
        max_candidate_nodes: 4,
        max_candidate_edges: 2,
        max_depth: 1,
        max_relation_paths: 1,
    };
    let deep_edges = [
        call_edge("e1", "a", "mid"),
        call_edge("e2", "mid", "b"),
        call_edge("e3", "b", "c"),
    ];
    let truncated = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: "Relate A, B, and C through wrappers",
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &seeds,
            relations: &deep_edges,
        },
        tight,
    );
    // With depth 1, A↔B through mid may be missing; limits may also truncate.
    assert!(
        !truncated.uncovered.is_empty()
            || truncated
                .objectives
                .iter()
                .any(|o| o.kind == RepositoryEvidenceObjectiveKind::RelationPath),
        "partial graphs must either select retained paths or report gaps: {truncated:?}"
    );
}

#[test]
fn six_language_front_ends_share_relation_objective_shape() {
    let languages = [
        ("src/main.rs", "lib/util.rs"),
        ("pkg/main.py", "pkg/util.py"),
        ("src/Main.java", "src/Util.java"),
        ("src/main.ts", "src/util.ts"),
        ("src/Main.kt", "src/Util.kt"),
        ("cmd/main.go", "internal/util.go"),
    ];
    let mut signatures = Vec::new();
    for (left_path, right_path) in languages {
        let seeds = [
            citation("left", "Left.run", left_path, NodeKind::FUNCTION),
            citation("right", "Right.run", right_path, NodeKind::FUNCTION),
        ];
        let edges = [call_edge("edge", "left", "right")];
        let plan = build_repository_evidence_plan(
            RepositoryEvidenceInput {
                question: "Trace Left.run calling Right.run",
                task_class: PacketTaskClassDto::RouteTracing,
                seeds: &seeds,
                relations: &edges,
            },
            DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
        );
        signatures.push(objective_signature(&plan));
    }
    assert!(
        signatures.windows(2).all(|w| w[0] == w[1]),
        "language path spellings must not change repository objective shape: {signatures:?}"
    );
}

#[test]
fn seed_plan_paraphrase_keeps_explicit_path_anchors() {
    let questions = [
        "Inspect src/alpha.rs and src/beta.rs relationship",
        "Look at the relationship in src/alpha.rs with src/beta.rs",
    ];
    for question in questions {
        let plan = build_packet_plan_with_extra(question, None, PacketBudgetModeDto::Standard, &[]);
        assert!(
            plan.queries
                .iter()
                .any(|q| q.query.contains("src/alpha.rs")),
            "missing path seed for {question}: {:?}",
            plan.queries
        );
        assert!(
            plan.queries.iter().any(|q| q.query.contains("src/beta.rs")),
            "missing path seed for {question}: {:?}",
            plan.queries
        );
        assert!(
            !plan.queries.iter().any(|q| {
                let purpose = q.purpose.to_ascii_lowercase();
                purpose.contains("flow-role")
            }),
            "paraphrase must not revive flow-role taxonomy queries"
        );
    }
}

#[test]
fn frozen_planner_limits_match_phase4_receipt() {
    assert_eq!(DEFAULT_REPOSITORY_EVIDENCE_LIMITS.max_seed_nodes, 12);
    assert_eq!(DEFAULT_REPOSITORY_EVIDENCE_LIMITS.max_candidate_nodes, 256);
    assert_eq!(DEFAULT_REPOSITORY_EVIDENCE_LIMITS.max_candidate_edges, 512);
    assert_eq!(DEFAULT_REPOSITORY_EVIDENCE_LIMITS.max_depth, 4);
    assert_eq!(DEFAULT_REPOSITORY_EVIDENCE_LIMITS.max_relation_paths, 32);
}
